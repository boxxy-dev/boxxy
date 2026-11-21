//! Unified `BoxxyAgent` wrapper and agent construction factories.

use crate::catalog::enrich_usage_with_endpoints_cost;
use crate::credentials::AiCredentials;
use crate::hook::ModelContextHook;
use crate::providers::{build_provider_model_with_options, configure_agent_builder};
use crate::search::{append_citations_to_output, attach_native_search};
use boxxy_model_selection::{ModelProvider, OpenAiCompatibleEndpoint};
use futures_util::StreamExt as _;
use rig::agent::{Agent, MultiTurnStreamItem, PromptResponse};
use rig::completion::{PromptError, Usage};
use rig::message::{AssistantContent, Message};
use rig::streaming::{Item, PartKind, StreamEvent};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const STREAM_EMIT_INTERVAL: Duration = Duration::from_millis(45);

/// Tuple returned by [`BoxxyAgent`] turn methods: `(formatted_output, enriched_usage, committed_messages)`.
pub type AgentTurnOutput = (String, Option<Usage>, Option<Vec<Message>>);

enum BoxxyAgentInner {
    Ready(Box<Agent>),
    Error(String),
}

struct BoxxyAgentState {
    inner: BoxxyAgentInner,
    provider: Option<ModelProvider>,
    endpoints: Vec<OpenAiCompatibleEndpoint>,
}

/// Thread-safe, `O(1)`-cloneable AI agent wrapper shared across Boxxy's zero-tool turns,
/// background memory consolidators, session title generators, and script generators.
#[derive(Clone)]
pub struct BoxxyAgent {
    state: Arc<BoxxyAgentState>,
}

impl BoxxyAgent {
    /// Returns `true` if a model was configured and the underlying [`Agent`] is ready.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        matches!(self.state.inner, BoxxyAgentInner::Ready(_))
    }

    /// Returns the [`ModelProvider`] backing this agent, if configured.
    #[must_use]
    pub fn provider(&self) -> Option<&ModelProvider> {
        self.state.provider.as_ref()
    }

    fn ready_agent(&self) -> Result<&Agent, &str> {
        match &self.state.inner {
            BoxxyAgentInner::Ready(agent) => Ok(agent),
            BoxxyAgentInner::Error(msg) => Err(msg.as_str()),
        }
    }

    fn provider_err(msg: &str) -> PromptError {
        PromptError::Provider(rig::error::ProviderError::Provider(msg.to_owned()))
    }

    fn finalize_response(&self, res: PromptResponse) -> AgentTurnOutput {
        let raw_output = res.output();
        let output = append_citations_to_output(&res.messages, &raw_output);
        if boxxy_sys_utils::env::debug_context() {
            log::info!(
                target: "model-context",
                "\n=== MODEL RESPONSE ===\n{output}\n======================\n"
            );
        }
        let usage = enrich_usage_with_endpoints_cost(
            res.usage,
            &self.state.provider,
            &self.state.endpoints,
        );
        (output, Some(usage), Some(res.messages))
    }

    /// Executes a turn with conversation `history`, enriching token cost and URL citations.
    ///
    /// # Errors
    /// Returns [`PromptError`] if no model is configured or if the provider request fails.
    #[expect(
        clippy::result_large_err,
        reason = "PromptError is Rig's canonical error type returned across async turn boundaries"
    )]
    pub async fn chat<P: Into<Message> + Send>(
        &self,
        prompt: P,
        history: Vec<Message>,
    ) -> Result<AgentTurnOutput, PromptError> {
        let agent = self.ready_agent().map_err(Self::provider_err)?;
        let msg = prompt.into();

        match agent.prompt(msg).history(history).await {
            Ok(res) => Ok(self.finalize_response(res)),
            Err(e) => {
                log_debug_error(&e);
                Err(e)
            }
        }
    }

    /// Streams a turn with conversation `history`, invoking `on_reasoning_done(thought, duration_ms)`
    /// as reasoning blocks finish and `on_text_delta(accumulated_text)` at ~45 ms intervals (plus a
    /// guaranteed final flush when streaming completes).
    ///
    /// # Errors
    /// Returns [`PromptError`] if no model is configured, the stream errors, or ends before `FinalResponse`.
    #[expect(
        clippy::result_large_err,
        reason = "PromptError is Rig's canonical error type returned across async turn boundaries"
    )]
    pub async fn chat_streaming<P, FReasoning, FText>(
        &self,
        prompt: P,
        history: Vec<Message>,
        mut on_reasoning_done: FReasoning,
        mut on_text_delta: FText,
    ) -> Result<AgentTurnOutput, PromptError>
    where
        P: Into<Message> + Send,
        FReasoning: FnMut(String, u64) + Send,
        FText: FnMut(&str) + Send,
    {
        let agent = self.ready_agent().map_err(Self::provider_err)?;
        let msg = prompt.into();

        let mut stream = agent.prompt(msg).history(history).stream();
        let mut reasoning_starts: HashMap<usize, Instant> = HashMap::new();
        let mut text_buf = String::new();
        let mut dirty_text = false;
        let mut emitted_any_text = false;
        let mut last_emit = Instant::now()
            .checked_sub(Duration::from_millis(100))
            .unwrap_or_else(Instant::now);
        let mut final_res: Option<PromptResponse> = None;

        while let Some(item_res) = stream.next().await {
            match item_res {
                Ok(MultiTurnStreamItem::StreamAssistantItem(Item::Event(ev))) => match ev {
                    StreamEvent::Start {
                        part,
                        kind: PartKind::Reasoning,
                        ..
                    } => {
                        reasoning_starts.insert(part.index(), Instant::now());
                    }
                    StreamEvent::Reasoning { part, .. } => {
                        // Fallback start timestamp for dialects that stream reasoning deltas
                        // without an explicit preceding `PartKind::Reasoning` start event.
                        reasoning_starts
                            .entry(part.index())
                            .or_insert_with(Instant::now);
                    }
                    StreamEvent::End {
                        part,
                        content: AssistantContent::Reasoning(r),
                    } => {
                        let dur_ms = reasoning_starts.remove(&part.index()).map_or(0, |t0| {
                            u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX)
                        });
                        on_reasoning_done(r.text, dur_ms);
                    }
                    StreamEvent::Text { text, .. } => {
                        text_buf.push_str(&text);
                        dirty_text = true;
                        if last_emit.elapsed() >= STREAM_EMIT_INTERVAL {
                            last_emit = Instant::now();
                            dirty_text = false;
                            emitted_any_text = true;
                            on_text_delta(&text_buf);
                        }
                    }
                    _ => {}
                },
                Ok(MultiTurnStreamItem::ModelTurnRetried { .. }) => {
                    reasoning_starts.clear();
                    text_buf.clear();
                    dirty_text = false;
                    if emitted_any_text {
                        emitted_any_text = false;
                        on_text_delta("");
                    }
                }
                Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                    final_res = Some(res);
                }
                Ok(_) => {}
                Err(e) => {
                    log_debug_error(&e);
                    return Err(e);
                }
            }
        }

        let Some(res) = final_res else {
            let err = PromptError::Provider(rig::error::ProviderError::Provider(
                "Model stream ended before FinalResponse".to_owned(),
            ));
            log_debug_error(&err);
            return Err(err);
        };

        // Flush any trailing text chunks buffered since the last 45 ms throttle tick.
        if dirty_text {
            on_text_delta(&text_buf);
        }

        Ok(self.finalize_response(res))
    }

    /// Executes a single stateless prompt with empty conversation history.
    ///
    /// # Errors
    /// Returns [`PromptError`] if no model is configured or if the provider request fails.
    #[expect(
        clippy::result_large_err,
        reason = "PromptError is Rig's canonical error type returned across async turn boundaries"
    )]
    pub async fn prompt(&self, prompt: &str) -> Result<AgentTurnOutput, PromptError> {
        self.chat(prompt, Vec::new()).await
    }
}

fn log_debug_error(err: &PromptError) {
    if boxxy_sys_utils::env::debug_context() {
        log::info!(
            target: "model-context",
            "\n=== MODEL ERROR ===\n{err:?}\n===================\n"
        );
    }
}

/// Configuration options for [`create_agent_with_config`].
#[derive(Debug, Clone, Default)]
pub struct AgentOptions {
    pub enable_native_search: bool,
    pub stateless: bool,
    pub session_cache_key: Option<String>,
}

/// Instantiates a [`BoxxyAgent`] with portable caching and reasoning options (native search disabled).
#[must_use]
pub fn create_agent(
    provider: &Option<ModelProvider>,
    creds: &AiCredentials,
    system_prompt: &str,
) -> BoxxyAgent {
    create_agent_with_options(provider, creds, system_prompt, false)
}

/// Instantiates a [`BoxxyAgent`] with portable caching, reasoning, `ModelContextHook` observability,
/// and optional provider-native web search.
#[must_use]
pub fn create_agent_with_options(
    provider: &Option<ModelProvider>,
    creds: &AiCredentials,
    system_prompt: &str,
    enable_native_search: bool,
) -> BoxxyAgent {
    create_agent_with_config(
        provider,
        creds,
        system_prompt,
        &AgentOptions {
            enable_native_search,
            stateless: false,
            session_cache_key: None,
        },
    )
}

/// Instantiates a [`BoxxyAgent`] with full control over native search, stateless `/incognito`
/// wire configuration (`"store": false` and cache suppression), and session cache routing.
#[must_use]
pub fn create_agent_with_config(
    provider: &Option<ModelProvider>,
    creds: &AiCredentials,
    system_prompt: &str,
    options: &AgentOptions,
) -> BoxxyAgent {
    let Some(p) = provider.as_ref() else {
        return BoxxyAgent {
            state: Arc::new(BoxxyAgentState {
                inner: BoxxyAgentInner::Error(
                    "No AI model selected. Please configure your models in Preferences -> APIs -> Models Selection."
                        .to_owned(),
                ),
                provider: None,
                endpoints: creds.openai_compatible_endpoints.clone(),
            }),
        };
    };

    let (model, _provider_name, _model_name, reasoning) =
        build_provider_model_with_options(p, creds, options.stateless);

    let mut builder = model
        .into_agent_builder()
        .preamble(system_prompt)
        .add_hook(ModelContextHook::new(system_prompt));

    builder = configure_agent_builder(
        builder,
        p,
        reasoning,
        &creds.openai_compatible_endpoints,
        options.session_cache_key.as_deref(),
        options.stateless,
    );

    if options.enable_native_search {
        builder = attach_native_search(builder, p);
    }

    BoxxyAgent {
        state: Arc::new(BoxxyAgentState {
            inner: BoxxyAgentInner::Ready(Box::new(builder.build())),
            provider: Some(p.clone()),
            endpoints: creds.openai_compatible_endpoints.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_model_selection::{CustomModelConfig, OpenAiApiRoute};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn chat_streaming_flushes_trailing_text_and_tracks_reasoning_and_cost() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept client");
            let mut req_buf = [0_u8; 4096];
            let _ = socket.read(&mut req_buf).await;

            // Emit SSE chunks rapidly (< 45 ms apart) with reasoning + split text + usage
            // and `[DONE]` without `finish_reason` to exercise `done_without_finish_reason`.
            let sse_body = concat!(
                "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"Step 1\"}}]}\n\n",
                "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello \"}}]}\n\n",
                "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"world!\"}}],\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":500,\"total_tokens\":1500}}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse_body.len(),
                sse_body
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write sse");
        });

        let mut ep = OpenAiCompatibleEndpoint::preset_local_server();
        ep.name = "MockServer".to_string();
        ep.base_url = format!("http://{addr}/v1/");
        ep.zero_cost = false;
        ep.models.push(CustomModelConfig {
            id: "mock-model".to_string(),
            display_name: "Mock Model".to_string(),
            route: OpenAiApiRoute::ChatCompletions,
            context_window: Some(32_768),
            thinking_levels: Vec::new(),
            input_cost_per_1m: Some(2.0),
            output_cost_per_1m: Some(4.0),
        });

        let creds = AiCredentials::default().with_custom_endpoints(vec![ep]);
        let provider = Some(ModelProvider::OpenAiCompatible {
            provider_name: "MockServer".to_string(),
            model_id: "mock-model".to_string(),
            thinking: None,
        });

        let agent = create_agent(&provider, &creds, "Test system prompt");
        assert!(agent.is_ready());

        let mut captured_thoughts = Vec::new();
        let mut last_streamed_text = String::new();

        let (output, usage_opt, messages_opt) = agent
            .chat_streaming(
                "Ping",
                Vec::new(),
                |thought, _dur_ms| captured_thoughts.push(thought),
                |delta_snapshot| {
                    delta_snapshot.clone_into(&mut last_streamed_text);
                },
            )
            .await
            .expect("streaming call succeeds");

        server.await.expect("server task finishes");

        assert_eq!(output, "Hello world!");
        // Trailing "world!" arrived within 45 ms of "Hello " and must still be flushed!
        assert_eq!(last_streamed_text, "Hello world!");
        assert_eq!(captured_thoughts, vec!["Step 1".to_string()]);
        assert!(messages_opt.is_some_and(|m| !m.is_empty()));

        let usage = usage_opt.expect("usage returned");
        assert_eq!(usage.input_tokens, Some(1_000));
        assert_eq!(usage.output_tokens, Some(500));
        // 1,000 * $2/1M ($0.002) + 500 * $4/1M ($0.002) = $0.004
        let total_cost = usage.cost.expect("custom pricing applied").total;
        assert!((total_cost - 0.004).abs() < 1e-9);
    }

    #[tokio::test]
    async fn stateless_responses_agent_sends_store_false_and_scopes_finish_reasons() {
        use crate::providers::needs_relaxed_finish_reasons;
        use boxxy_model_selection::{AnthropicModel, GeminiModel, OpenAiModel};

        assert!(!needs_relaxed_finish_reasons(&ModelProvider::Gemini(
            GeminiModel::Flash3_8,
            None
        )));
        assert!(!needs_relaxed_finish_reasons(&ModelProvider::Anthropic(
            AnthropicModel::ClaudeSonnet5,
            None
        )));
        assert!(!needs_relaxed_finish_reasons(&ModelProvider::OpenAi(
            OpenAiModel::Gpt6Astra,
            None
        )));
        assert!(needs_relaxed_finish_reasons(&ModelProvider::Ollama(
            "qwen3:8b".to_string()
        )));
        assert!(needs_relaxed_finish_reasons(&ModelProvider::OpenRouter(
            "anthropic/claude-sonnet-5".to_string()
        )));
        assert!(needs_relaxed_finish_reasons(
            &ModelProvider::OpenAiCompatible {
                provider_name: "Local".to_string(),
                model_id: "m".to_string(),
                thinking: None,
            }
        ));

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept client");
            let mut req_buf = vec![0_u8; 8192];
            let n = socket.read(&mut req_buf).await.expect("read request");
            let raw_req = String::from_utf8_lossy(&req_buf[..n]).into_owned();

            let json_body = "{\"id\":\"resp_1\",\"object\":\"response\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ephemeral ok\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":10,\"output_tokens\":2,\"total_tokens\":12}}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                json_body.len(),
                json_body
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
            raw_req
        });

        let mut ep = OpenAiCompatibleEndpoint::preset_local_server();
        ep.name = "MockResponses".to_string();
        ep.base_url = format!("http://{addr}/v1");
        ep.models
            .push(CustomModelConfig::new("gpt-6.2-sol").with_route(OpenAiApiRoute::Responses));

        let creds = AiCredentials::default().with_custom_endpoints(vec![ep]);
        let provider = Some(ModelProvider::OpenAiCompatible {
            provider_name: "MockResponses".to_string(),
            model_id: "gpt-6.2-sol".to_string(),
            thinking: None,
        });

        let agent = create_agent_with_config(
            &provider,
            &creds,
            "Incognito instructions",
            &AgentOptions {
                enable_native_search: false,
                stateless: true,
                session_cache_key: Some("must-not-be-sent".to_string()),
            },
        );

        let (reply, _, _) = agent.prompt("Hello").await.expect("stateless prompt");
        assert_eq!(reply, "ephemeral ok");

        let raw_req = server.await.expect("server captured request");
        let (_, body_str) = raw_req.split_once("\r\n\r\n").expect("HTTP body");
        let parsed: serde_json::Value = serde_json::from_str(body_str).expect("JSON body");
        assert_eq!(parsed.get("store"), Some(&serde_json::json!(false)));
        assert!(parsed.get("prompt_cache_key").is_none());
    }

    async fn spawn_mock_codex_sse_server() -> (std::net::SocketAddr, tokio::task::JoinHandle<String>)
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept client");
            let mut req_buf = Vec::with_capacity(8192);
            let mut chunk = [0_u8; 4096];
            loop {
                let n = socket.read(&mut chunk).await.expect("read request");
                if n == 0 {
                    break;
                }
                req_buf.extend_from_slice(&chunk[..n]);
                if let Some(hdr_end) = req_buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let hdr_str = String::from_utf8_lossy(&req_buf[..hdr_end]);
                    let content_len = hdr_str
                        .lines()
                        .find_map(|line| {
                            let (k, v) = line.split_once(':')?;
                            k.trim()
                                .eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if req_buf.len() >= hdr_end + 4 + content_len {
                        break;
                    }
                }
            }
            let raw_req = String::from_utf8_lossy(&req_buf).into_owned();

            let sse_body = concat!(
                "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_codex_1\",\"object\":\"response\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
                "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Codex OAuth ok\"}\n\n",
                "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_codex_1\",\"object\":\"response\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Codex OAuth ok\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":120,\"output_tokens\":15,\"total_tokens\":135}}}\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse_body.len(),
                sse_body
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write codex sse");
            raw_req
        });

        (addr, server)
    }

    #[tokio::test]
    async fn chatgpt_signed_in_model_streams_codex_sse_with_account_id_and_reasoning() {
        const CHILD_ENV: &str = "BOXXY_TEST_CHATGPT_SIGNED_IN_CHILD";

        if std::env::var_os(CHILD_ENV).is_none() {
            let (addr, server) = spawn_mock_codex_sse_server().await;

            let tmp = tempfile::tempdir().expect("tempdir");
            let auth_dir = tmp.path().join("auth");
            std::fs::create_dir_all(&auth_dir).expect("create auth dir");
            let future_exp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("epoch")
                .as_secs()
                + 3600;
            std::fs::write(
                auth_dir.join("chatgpt.json"),
                serde_json::json!({
                    "access_token": "synthetic-oauth-access-token",
                    "refresh_token": "synthetic-oauth-refresh-token",
                    "account_id": "acct_codex_test",
                    "expires_at": future_exp
                })
                .to_string(),
            )
            .expect("write chatgpt.json");

            let exe = std::env::current_exe().expect("test exe");
            let out = tokio::process::Command::new(exe)
                .args([
                    "--exact",
                    "agent::tests::chatgpt_signed_in_model_streams_codex_sse_with_account_id_and_reasoning",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .env("BOXXY_CONFIG_DIR", tmp.path())
                .env("CHATGPT_API_BASE", format!("http://{addr}/backend-api/codex"))
                .output()
                .await
                .expect("run child test");

            assert!(
                out.status.success(),
                "child failed:\n{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );

            let raw_req = server.await.expect("server request");
            let (headers_str, body_str) = raw_req.split_once("\r\n\r\n").expect("HTTP split");
            let headers_lower = headers_str.to_ascii_lowercase();
            assert!(headers_lower.contains("authorization: bearer synthetic-oauth-access-token"));
            assert!(headers_lower.contains("chatgpt-account-id: acct_codex_test"));
            assert!(headers_lower.contains("session-id: sess-codex-42"));

            let parsed: serde_json::Value = serde_json::from_str(body_str).expect("JSON body");
            assert_eq!(parsed.get("store"), Some(&serde_json::json!(false)));
            assert_eq!(
                parsed.get("prompt_cache_key"),
                Some(&serde_json::json!("sess-codex-42"))
            );
            assert_eq!(
                parsed.get("reasoning"),
                Some(&serde_json::json!({"effort": "high", "summary": "auto"}))
            );
            return;
        }

        let _ = crate::refresh_chatgpt_auth_status();
        let provider = Some(ModelProvider::ChatGpt(
            boxxy_model_selection::ChatGptModel::new("gpt-6.2-sol"),
            Some(boxxy_model_selection::ThinkingLevel::High),
        ));
        let creds = AiCredentials::default();
        let agent = create_agent_with_config(
            &provider,
            &creds,
            "Codex system instructions",
            &AgentOptions {
                enable_native_search: true,
                stateless: false,
                session_cache_key: Some("sess-codex-42".to_string()),
            },
        );

        let mut streamed = String::new();
        let (reply, usage_opt, _) = agent
            .chat_streaming(
                "Ping Codex",
                Vec::new(),
                |_, _| {},
                |delta| delta.clone_into(&mut streamed),
            )
            .await
            .expect("codex chat_streaming");

        assert_eq!(reply, "Codex OAuth ok");
        assert_eq!(streamed, "Codex OAuth ok");
        let usage = usage_opt.expect("usage present");
        assert_eq!(usage.total_tokens, Some(135));
        assert!(usage.cost.is_none());
    }
}
