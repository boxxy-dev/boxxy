//! Unified `BoxxyAgent` wrapper and agent construction factories.

use crate::catalog::enrich_usage_with_endpoints_cost;
use crate::credentials::AiCredentials;
use crate::hook::ModelContextHook;
use crate::providers::build_provider_model;
use crate::search::{append_citations_to_output, attach_native_search};
use boxxy_model_selection::{ModelProvider, OpenAiCompatibleEndpoint};
use futures_util::StreamExt as _;
use rig::agent::{Agent, AgentBuilder, MultiTurnStreamItem, PromptResponse};
use rig::completion::{CacheRetention, OnUnsupported, PromptError, Usage};
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

    let (model, _provider_name, _model_name, reasoning) = build_provider_model(p, creds);

    let mut builder = AgentBuilder::new(model)
        .preamble(system_prompt)
        .cache(CacheRetention::Short)
        .on_unsupported(OnUnsupported::Ignore)
        .accept_unknown_finish_reasons(true)
        .add_hook(ModelContextHook::new(system_prompt));

    if let Some(reasoning) = reasoning {
        builder = builder.reasoning(reasoning);
    }

    if enable_native_search {
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
}
