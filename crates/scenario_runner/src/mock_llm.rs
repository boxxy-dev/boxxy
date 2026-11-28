//! Deterministic, scriptable in-process OpenAI-compatible HTTP/SSE server for headless E2E tests.
//!
//! Binds `127.0.0.1:0` on Tokio, parses full `POST /v1/chat/completions` payloads (including
//! multi-kilobyte system prompts and tool schemas), separates main agent turns from `/btw`
//! side-channel queries and background memory/title calls, and supports zero-sleep `TurnGate`
//! synchronization barriers.

use crate::config_env::MOCK_MEMORY_MODEL_ID;
use anyhow::{Context, Result, anyhow};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

const MAX_HTTP_BODY_BYTES: usize = 4 * 1024 * 1024;

static CALL_ID_SEQ: AtomicU64 = AtomicU64::new(1);

/// Deterministic synchronization barrier allowing a test to pause an in-flight LLM turn,
/// inspect or mutate session state (queue prompts, run `/btw`, send `Abort`), and resume cleanly.
#[derive(Clone, Default)]
pub struct TurnGate {
    entered: Arc<AtomicBool>,
    entered_notify: Arc<Notify>,
    released: Arc<AtomicBool>,
    release_notify: Arc<Notify>,
}

impl std::fmt::Debug for TurnGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnGate")
            .field("entered", &self.entered.load(Ordering::Relaxed))
            .field("released", &self.released.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl TurnGate {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Waits until the mock LLM server has accepted the request for this gated step.
    pub async fn wait_entered(&self) {
        loop {
            let notified = self.entered_notify.notified();
            if self.entered.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Releases the gated step so the mock server streams its scripted response.
    pub fn release(&self) {
        self.released.store(true, Ordering::Release);
        self.release_notify.notify_waiters();
    }

    fn mark_entered(&self) {
        self.entered.store(true, Ordering::Release);
        self.entered_notify.notify_waiters();
    }

    async fn wait_released(&self) {
        loop {
            let notified = self.release_notify.notified();
            if self.released.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

/// Scripted tool invocation emitted by [`MockLlmServer`].
#[derive(Debug, Clone)]
pub struct MockToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

impl MockToolCall {
    #[must_use]
    pub fn new(name: impl Into<String>, arguments: &Value) -> Self {
        let seq = CALL_ID_SEQ.fetch_add(1, Ordering::Relaxed);
        Self {
            id: format!("call_mock_{seq}"),
            name: name.into(),
            arguments_json: serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string()),
        }
    }
}

pub type DynamicStepFn = Arc<dyn Fn(&RecordedChatRequest) -> MockStep + Send + Sync>;

/// Single scripted completion response queued on [`MockLlmServer`].
#[derive(Clone)]
pub enum MockStep {
    Text {
        thought: Option<String>,
        text: String,
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    ToolCalls {
        thought: Option<String>,
        calls: Vec<MockToolCall>,
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    HttpError {
        status: u16,
        message: String,
    },
    Gated {
        gate: TurnGate,
        inner: Box<Self>,
    },
    Dynamic(DynamicStepFn),
}

impl std::fmt::Debug for MockStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text {
                thought,
                text,
                prompt_tokens,
                completion_tokens,
            } => f
                .debug_struct("Text")
                .field("thought", thought)
                .field("text", text)
                .field("prompt_tokens", prompt_tokens)
                .field("completion_tokens", completion_tokens)
                .finish(),
            Self::ToolCalls {
                thought,
                calls,
                prompt_tokens,
                completion_tokens,
            } => f
                .debug_struct("ToolCalls")
                .field("thought", thought)
                .field("calls", calls)
                .field("prompt_tokens", prompt_tokens)
                .field("completion_tokens", completion_tokens)
                .finish(),
            Self::HttpError { status, message } => f
                .debug_struct("HttpError")
                .field("status", status)
                .field("message", message)
                .finish(),
            Self::Gated { gate, inner } => f
                .debug_struct("Gated")
                .field("gate", gate)
                .field("inner", inner)
                .finish(),
            Self::Dynamic(_) => f.write_str("Dynamic(..)"),
        }
    }
}

impl MockStep {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            thought: None,
            text: text.into(),
            prompt_tokens: 400,
            completion_tokens: 100,
        }
    }

    #[must_use]
    pub fn thought_and_text(thought: impl Into<String>, text: impl Into<String>) -> Self {
        Self::Text {
            thought: Some(thought.into()),
            text: text.into(),
            prompt_tokens: 500,
            completion_tokens: 150,
        }
    }

    #[must_use]
    pub fn with_usage(mut self, prompt_tokens: u64, completion_tokens: u64) -> Self {
        match &mut self {
            Self::Text {
                prompt_tokens: p,
                completion_tokens: c,
                ..
            }
            | Self::ToolCalls {
                prompt_tokens: p,
                completion_tokens: c,
                ..
            } => {
                *p = prompt_tokens;
                *c = completion_tokens;
            }
            Self::Gated { inner, .. } => {
                let updated = (**inner).clone().with_usage(prompt_tokens, completion_tokens);
                **inner = updated;
            }
            Self::HttpError { .. } | Self::Dynamic(_) => {}
        }
        self
    }

    #[must_use]
    pub fn dynamic<F>(builder: F) -> Self
    where
        F: Fn(&RecordedChatRequest) -> Self + Send + Sync + 'static,
    {
        Self::Dynamic(Arc::new(builder))
    }

    #[must_use]
    pub fn tool_call(name: impl Into<String>, arguments: &Value) -> Self {
        Self::ToolCalls {
            thought: None,
            calls: vec![MockToolCall::new(name, arguments)],
            prompt_tokens: 450,
            completion_tokens: 80,
        }
    }

    #[must_use]
    pub const fn tool_calls(calls: Vec<MockToolCall>) -> Self {
        Self::ToolCalls {
            thought: None,
            calls,
            prompt_tokens: 500,
            completion_tokens: 120,
        }
    }

    #[must_use]
    pub fn http_error(status: u16, message: impl Into<String>) -> Self {
        Self::HttpError {
            status,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn gated(gate: TurnGate, inner: Self) -> Self {
        Self::Gated {
            gate,
            inner: Box::new(inner),
        }
    }
}

/// Captured wire payload of a `POST /v1/chat/completions` call for test assertions.
#[derive(Debug, Clone)]
pub struct RecordedChatRequest {
    pub model: String,
    pub stream: bool,
    pub authorization: Option<String>,
    pub messages: Vec<Value>,
    pub tools: Vec<Value>,
    pub tool_choice: Option<Value>,
    pub raw_body: Value,
}

impl RecordedChatRequest {
    /// Concatenates all message text segments in this request for substring assertions.
    #[must_use]
    pub fn flattened_message_text(&self) -> String {
        let mut buf = String::new();
        for msg in &self.messages {
            if let Some(s) = msg.get("content").and_then(Value::as_str) {
                buf.push_str(s);
                buf.push('\n');
            } else if let Some(arr) = msg.get("content").and_then(Value::as_array) {
                for part in arr {
                    if let Some(t) = part.get("text").and_then(Value::as_str) {
                        buf.push_str(t);
                        buf.push('\n');
                    }
                }
            }
        }
        buf
    }

    /// Returns concatenated text of all `system` / `developer` role messages in this request.
    #[must_use]
    pub fn system_prompt_text(&self) -> String {
        let mut buf = String::new();
        for msg in &self.messages {
            let role = msg.get("role").and_then(Value::as_str).unwrap_or_default();
            if role == "system" || role == "developer" {
                if let Some(s) = msg.get("content").and_then(Value::as_str) {
                    buf.push_str(s);
                    buf.push('\n');
                } else if let Some(arr) = msg.get("content").and_then(Value::as_array) {
                    for part in arr {
                        if let Some(t) = part.get("text").and_then(Value::as_str) {
                            buf.push_str(t);
                            buf.push('\n');
                        }
                    }
                }
            }
        }
        buf
    }

    /// Returns concatenated text of all non-system messages (`user`, `assistant`, `tool`) in this request.
    #[must_use]
    pub fn conversation_text(&self) -> String {
        let mut buf = String::new();
        for msg in &self.messages {
            let role = msg.get("role").and_then(Value::as_str).unwrap_or_default();
            if role == "system" || role == "developer" {
                continue;
            }
            if let Some(s) = msg.get("content").and_then(Value::as_str) {
                buf.push_str(s);
                buf.push('\n');
            } else if let Some(arr) = msg.get("content").and_then(Value::as_array) {
                for part in arr {
                    if let Some(t) = part.get("text").and_then(Value::as_str) {
                        buf.push_str(t);
                        buf.push('\n');
                    }
                }
            }
        }
        buf
    }

    /// Returns registered function tool names sent in `tools`.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter_map(|t| {
                t.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect()
    }
}

#[derive(Default)]
struct ServerQueues {
    main_steps: VecDeque<MockStep>,
    btw_steps: VecDeque<MockStep>,
    aux_steps: VecDeque<String>,
    main_requests: Vec<RecordedChatRequest>,
    btw_requests: Vec<RecordedChatRequest>,
    aux_requests: Vec<RecordedChatRequest>,
}

/// Local scriptable OpenAI-compatible HTTP server.
pub struct MockLlmServer {
    addr: SocketAddr,
    queues: Arc<Mutex<ServerQueues>>,
    accept_task: JoinHandle<()>,
}

impl MockLlmServer {
    /// Starts a new local mock LLM server bound to an ephemeral `127.0.0.1` port.
    ///
    /// # Errors
    /// Returns an error if binding the loopback TCP listener fails.
    pub async fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind MockLlmServer TCP listener")?;
        let addr = listener
            .local_addr()
            .context("resolve MockLlmServer local_addr")?;
        let queues = Arc::new(Mutex::new(ServerQueues::default()));
        let queues_bg = Arc::clone(&queues);

        let accept_task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let q = Arc::clone(&queues_bg);
                tokio::spawn(async move {
                    let _ = handle_connection(stream, q).await;
                });
            }
        });

        Ok(Self {
            addr,
            queues,
            accept_task,
        })
    }

    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    /// Enqueues a scripted response step for the primary `ClawAgent` conversation loop.
    pub fn push_step(&self, step: MockStep) {
        self.queues.lock().main_steps.push_back(step);
    }

    /// Enqueues multiple scripted response steps in FIFO order.
    pub fn push_steps(&self, steps: impl IntoIterator<Item = MockStep>) {
        let mut guard = self.queues.lock();
        for step in steps {
            guard.main_steps.push_back(step);
        }
    }

    /// Enqueues a scripted response specifically for `/btw` side-channel queries (`tool_choice: "none"`).
    pub fn push_btw_step(&self, step: MockStep) {
        self.queues.lock().btw_steps.push_back(step);
    }

    /// Enqueues an explicit plain-text response for background auxiliary calls (title / memory flush / wake).
    pub fn push_aux_text(&self, text: impl Into<String>) {
        self.queues.lock().aux_steps.push_back(text.into());
    }

    #[must_use]
    pub fn main_requests(&self) -> Vec<RecordedChatRequest> {
        self.queues.lock().main_requests.clone()
    }

    #[must_use]
    pub fn btw_requests(&self) -> Vec<RecordedChatRequest> {
        self.queues.lock().btw_requests.clone()
    }

    #[must_use]
    pub fn aux_requests(&self) -> Vec<RecordedChatRequest> {
        self.queues.lock().aux_requests.clone()
    }

    #[must_use]
    pub fn remaining_main_steps(&self) -> usize {
        self.queues.lock().main_steps.len()
    }
}

impl Drop for MockLlmServer {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

async fn read_http_json_body(
    stream: &mut TcpStream,
) -> Result<(String, Option<String>, Value)> {
    let mut buf = Vec::with_capacity(16 * 1024);
    let mut chunk = [0_u8; 8192];
    let header_end;
    let mut content_len = 0_usize;
    let mut authorization = None;

    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(anyhow!("client closed connection before HTTP headers"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_HTTP_BODY_BYTES {
            return Err(anyhow!("HTTP request exceeded safety limit"));
        }
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = pos;
            let hdr_str = String::from_utf8_lossy(&buf[..header_end]);
            for line in hdr_str.lines().skip(1) {
                if let Some((k, v)) = line.split_once(':') {
                    let key = k.trim();
                    if key.eq_ignore_ascii_case("content-length") {
                        content_len = v.trim().parse::<usize>().unwrap_or(0);
                    } else if key.eq_ignore_ascii_case("authorization") {
                        authorization = Some(v.trim().to_string());
                    }
                }
            }
            break;
        }
    }

    let body_start = header_end + 4;
    let target_total = body_start.saturating_add(content_len);
    if target_total > MAX_HTTP_BODY_BYTES {
        return Err(anyhow!("Content-Length exceeded safety limit"));
    }

    while buf.len() < target_total {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }

    let req_line = String::from_utf8_lossy(&buf[..header_end])
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();

    if content_len == 0 {
        return Ok((req_line, authorization, json!({})));
    }
    let slice_end = target_total.min(buf.len());
    let parsed: Value = serde_json::from_slice(&buf[body_start..slice_end])
        .context("deserialize /v1/chat/completions JSON body")?;
    Ok((req_line, authorization, parsed))
}

fn is_aux_request(req: &RecordedChatRequest) -> bool {
    if req.model == MOCK_MEMORY_MODEL_ID {
        return true;
    }
    let flat = req.flattened_message_text();
    flat.contains("You are a conversation title generator")
}

fn is_btw_request(req: &RecordedChatRequest) -> bool {
    if req.tool_choice.as_ref().and_then(Value::as_str) == Some("none") {
        return true;
    }
    req.flattened_message_text()
        .contains("[SIDE QUESTION (/btw) — EPHEMERAL]")
}

async fn handle_connection(mut stream: TcpStream, queues: Arc<Mutex<ServerQueues>>) -> Result<()> {
    let (req_line, authorization, body) = read_http_json_body(&mut stream).await?;

    // Support `/v1/models` health discovery cleanly if queried.
    if req_line.starts_with("GET ") && req_line.contains("/models") {
        let payload = serde_json::to_string(&json!({
            "object": "list",
            "data": [{ "id": "mock-claw-model", "object": "model" }]
        }))?;
        return write_http_response(&mut stream, 200, "application/json", &payload).await;
    }

    let recorded = RecordedChatRequest {
        model: body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        stream: body.get("stream").and_then(Value::as_bool).unwrap_or(false),
        authorization,
        messages: body
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        tools: body
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        tool_choice: body.get("tool_choice").cloned(),
        raw_body: body,
    };

    let want_stream = recorded.stream;

    let req_snapshot = recorded.clone();
    let step = if is_aux_request(&recorded) {
        let text = {
            let mut q = queues.lock();
            let is_title = recorded
                .flattened_message_text()
                .contains("conversation title generator");
            q.aux_requests.push(recorded);
            q.aux_steps.pop_front().unwrap_or_else(|| {
                if is_title {
                    "Synthetic Session Summary".to_string()
                } else {
                    "NO_TECHNICAL_CHANGE".to_string()
                }
            })
        };
        MockStep::text(text).with_usage(50, 10)
    } else if is_btw_request(&recorded) {
        let popped = {
            let mut q = queues.lock();
            q.btw_requests.push(recorded);
            q.btw_steps.pop_front().or_else(|| q.main_steps.pop_front())
        };
        popped.unwrap_or_else(|| {
            MockStep::http_error(500, "MockLlmServer: no scripted /btw step queued")
        })
    } else {
        let popped = {
            let mut q = queues.lock();
            q.main_requests.push(recorded);
            q.main_steps.pop_front()
        };
        popped.unwrap_or_else(|| {
            MockStep::http_error(500, "MockLlmServer: no scripted main MockStep queued")
        })
    };

    respond_with_step(&mut stream, step, &req_snapshot, want_stream).await
}

async fn respond_with_step(
    stream: &mut TcpStream,
    mut step: MockStep,
    req: &RecordedChatRequest,
    want_stream: bool,
) -> Result<()> {
    loop {
        match step {
            MockStep::Gated { gate, inner } => {
                gate.mark_entered();
                gate.wait_released().await;
                step = *inner;
            }
            MockStep::Dynamic(builder) => {
                step = builder(req);
            }
            other => {
                step = other;
                break;
            }
        }
    }

    match step {
        MockStep::HttpError { status, message } => {
            let err_json = serde_json::to_string(&json!({
                "error": {
                    "message": message,
                    "type": "mock_server_error",
                    "code": status
                }
            }))?;
            write_http_response(stream, status, "application/json", &err_json).await
        }
        MockStep::Text {
            thought,
            text,
            prompt_tokens,
            completion_tokens,
        } => {
            respond_text(
                stream,
                thought,
                &text,
                prompt_tokens,
                completion_tokens,
                want_stream,
            )
            .await
        }
        MockStep::ToolCalls {
            thought,
            calls,
            prompt_tokens,
            completion_tokens,
        } => {
            respond_tool_calls(
                stream,
                thought,
                &calls,
                prompt_tokens,
                completion_tokens,
                want_stream,
            )
            .await
        }
        MockStep::Gated { .. } | MockStep::Dynamic(_) => unreachable!("unwrapped in loop above"),
    }
}

async fn respond_text(
    stream: &mut TcpStream,
    thought: Option<String>,
    text: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    want_stream: bool,
) -> Result<()> {
    if want_stream {
        let mut sse = String::new();
        let sent_role = thought.is_some_and(|reasoning| {
            let chunk = json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion.chunk",
                "choices": [{
                    "index": 0,
                    "delta": { "role": "assistant", "reasoning_content": reasoning },
                    "finish_reason": Value::Null
                }]
            });
            let _ = writeln!(sse, "data: {chunk}\n");
            true
        });

        let mut delta = serde_json::Map::new();
        if !sent_role {
            delta.insert("role".to_string(), json!("assistant"));
        }
        delta.insert("content".to_string(), json!(text));
        let content_chunk = json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": Value::Null
            }]
        });
        let _ = writeln!(sse, "data: {content_chunk}\n");

        let stop_chunk = json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": prompt_tokens.saturating_add(completion_tokens)
            }
        });
        let _ = writeln!(sse, "data: {stop_chunk}\n");
        sse.push_str("data: [DONE]\n\n");
        write_http_response(stream, 200, "text/event-stream", &sse).await
    } else {
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("assistant"));
        msg.insert("content".to_string(), json!(text));
        if let Some(reasoning) = thought {
            msg.insert("reasoning_content".to_string(), json!(reasoning));
        }
        let body = serde_json::to_string(&json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": msg,
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": prompt_tokens.saturating_add(completion_tokens)
            }
        }))?;
        write_http_response(stream, 200, "application/json", &body).await
    }
}

async fn respond_tool_calls(
    stream: &mut TcpStream,
    thought: Option<String>,
    calls: &[MockToolCall],
    prompt_tokens: u64,
    completion_tokens: u64,
    want_stream: bool,
) -> Result<()> {
    if want_stream {
        let mut sse = String::new();
        let mut role_delta = serde_json::Map::new();
        role_delta.insert("role".to_string(), json!("assistant"));
        if let Some(reasoning) = thought {
            role_delta.insert("reasoning_content".to_string(), json!(reasoning));
        }
        let init_chunk = json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "choices": [{
                "index": 0,
                "delta": role_delta,
                "finish_reason": Value::Null
            }]
        });
        let _ = writeln!(sse, "data: {init_chunk}\n");

        for (idx, call) in calls.iter().enumerate() {
            let call_chunk = json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion.chunk",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": idx,
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments_json
                            }
                        }]
                    },
                    "finish_reason": Value::Null
                }]
            });
            let _ = writeln!(sse, "data: {call_chunk}\n");
        }

        let finish_chunk = json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": prompt_tokens.saturating_add(completion_tokens)
            }
        });
        let _ = writeln!(sse, "data: {finish_chunk}\n");
        sse.push_str("data: [DONE]\n\n");
        write_http_response(stream, 200, "text/event-stream", &sse).await
    } else {
        let wire_calls: Vec<Value> = calls
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "type": "function",
                    "function": {
                        "name": c.name,
                        "arguments": c.arguments_json
                    }
                })
            })
            .collect();
        let body = serde_json::to_string(&json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": Value::Null,
                    "tool_calls": wire_calls
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": prompt_tokens.saturating_add(completion_tokens)
            }
        }))?;
        write_http_response(stream, 200, "application/json", &body).await
    }
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        429 => "Too Many Requests",
        _ => "Internal Server Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn turn_gate_coordinates_enter_and_release_deterministically() {
        let gate = TurnGate::new();
        let gate_worker = gate.clone();

        let worker = tokio::spawn(async move {
            gate_worker.mark_entered();
            gate_worker.wait_released().await;
            42_u32
        });

        gate.wait_entered().await;
        assert!(format!("{gate:?}").contains("entered: true"));
        gate.release();
        assert_eq!(worker.await.expect("join worker"), 42);
    }

    #[tokio::test]
    async fn mock_server_serves_models_and_records_requests() {
        let server = MockLlmServer::start().await.expect("start mock server");
        server.push_step(MockStep::text("pong").with_usage(12, 4));

        // Verify `/v1/models` health endpoint over raw loopback TCP.
        let addr_str = server
            .base_url()
            .trim_start_matches("http://")
            .trim_end_matches("/v1")
            .to_string();
        let mut sock = TcpStream::connect(&addr_str).await.expect("connect");
        sock.write_all(b"GET /v1/models HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
            .await
            .expect("write GET /v1/models");
        let mut resp = String::new();
        sock.read_to_string(&mut resp).await.expect("read response");
        assert!(resp.contains("200 OK"));
        assert!(resp.contains("mock-claw-model"));

        assert_eq!(server.remaining_main_steps(), 1);
    }
}
