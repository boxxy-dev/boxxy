//! Zero-overhead `AgentHook` for opt-in `BOXXY_DEBUG_CONTEXT` observability.

use rig::agent::hook::{
    AgentHook, CompletionCallAction, CompletionCallEvent, DispatchAction, DispatchEvent,
    HookContext,
};
use rig::wasm_compat::WasmCompatSend;
use std::borrow::Cow;
use std::fmt::Write as _;
use std::future::Future;

const MIN_BLOB_REDACT_LEN: usize = 256;
const BLOB_PREVIEW_LEN: usize = 32;

const fn is_base64_blob_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'_' | b'-')
}

/// Collapses contiguous base64 / ciphertext runs (`>= 256` bytes, such as `OpenAI` Responses
/// `encrypted_content` tokens or inline base64 image buffers) into a compact preview.
fn redact_debug_blobs(input: &str) -> Cow<'_, str> {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut first_blob: Option<(usize, usize)> = None;

    while i < bytes.len() {
        if is_base64_blob_byte(bytes[i]) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_base64_blob_byte(bytes[i]) {
                i += 1;
            }
            if i - start >= MIN_BLOB_REDACT_LEN {
                first_blob = Some((start, i));
                break;
            }
        } else {
            i += 1;
        }
    }

    let Some((first_start, first_end)) = first_blob else {
        return Cow::Borrowed(input);
    };

    let mut out = String::with_capacity(input.len().min(65_536));
    out.push_str(&input[..first_start]);
    append_redacted_run(&mut out, &input[first_start..first_end]);

    let mut cursor = first_end;
    let mut pos = first_end;
    while pos < bytes.len() {
        if is_base64_blob_byte(bytes[pos]) {
            let start = pos;
            pos += 1;
            while pos < bytes.len() && is_base64_blob_byte(bytes[pos]) {
                pos += 1;
            }
            if pos - start >= MIN_BLOB_REDACT_LEN {
                out.push_str(&input[cursor..start]);
                append_redacted_run(&mut out, &input[start..pos]);
                cursor = pos;
            }
        } else {
            pos += 1;
        }
    }

    if cursor < input.len() {
        out.push_str(&input[cursor..]);
    }
    Cow::Owned(out)
}

fn append_redacted_run(out: &mut String, run: &str) {
    // Every byte in `run` is guaranteed ASCII by `is_base64_blob_byte`.
    let preview = &run[..BLOB_PREVIEW_LEN];
    let omitted = run.len() - BLOB_PREVIEW_LEN;
    let _ = write!(out, "{preview}...<[{omitted} chars redacted]>");
}

/// Intercepts LLM completion requests and tool dispatches, logging full payloads to the
/// `model-context` target strictly when `--debug-context` / `BOXXY_DEBUG_CONTEXT=1` is active.
///
/// When context debugging is disabled (the default), both hook callbacks perform **zero**
/// message, history, or string clones.
#[derive(Debug, Clone)]
pub struct ModelContextHook {
    pub preamble: String,
}

impl ModelContextHook {
    #[must_use]
    pub fn new(preamble: impl Into<String>) -> Self {
        Self {
            preamble: preamble.into(),
        }
    }
}

impl AgentHook for ModelContextHook {
    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        event: CompletionCallEvent<'_>,
    ) -> impl Future<Output = CompletionCallAction> + WasmCompatSend {
        // Format synchronously from borrowed slices only when explicitly enabled so standard
        // turns never clone `self.preamble`, `event.prompt`, or `event.history`.
        let formatted_log = boxxy_sys_utils::env::debug_context().then(|| {
            let history_dbg = format!("{:#?}", event.history);
            let prompt_dbg = format!("{:#?}", event.prompt);
            format!(
                "\n=== MODEL CONTEXT SEND ===\nSYSTEM PROMPT:\n{}\n\nHISTORY:\n{}\n\nUSER PROMPT:\n{}\n==========================\n",
                self.preamble,
                redact_debug_blobs(&history_dbg),
                redact_debug_blobs(&prompt_dbg),
            )
        });

        async move {
            if let Some(payload) = formatted_log {
                log::info!(target: "model-context", "{payload}");
            }
            CompletionCallAction::Continue
        }
    }

    fn on_dispatch(
        &self,
        _ctx: &HookContext,
        event: DispatchEvent<'_>,
    ) -> impl Future<Output = DispatchAction> + WasmCompatSend {
        let formatted_log = boxxy_sys_utils::env::debug_context()
            .then(|| event.tool_name().zip(event.tool_args()))
            .flatten()
            .map(|(tool_name, args)| {
                format!(
                    "\n=== MODEL TOOL CALL ===\nTOOL: {tool_name}\nARGS: {}\n=======================\n",
                    redact_debug_blobs(args),
                )
            });

        async move {
            if let Some(payload) = formatted_log {
                log::info!(target: "model-context", "{payload}");
            }
            DispatchAction::Proceed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_long_ciphertext_and_base64_blobs_while_preserving_normal_text() {
        let normal = "User prompt with code:\nfn main() { println!(\"hello world\"); }\n";
        assert!(matches!(redact_debug_blobs(normal), Cow::Borrowed(_)));

        let blob = "AbCdEfGh01234567".repeat(25); // 400 synthetic ASCII base64 chars
        let debug_dump =
            format!("\"encrypted_content\": String(\"{blob}\"), \"role\": \"assistant\"");
        let redacted = redact_debug_blobs(&debug_dump);

        assert!(redacted.contains("AbCdEfGh01234567AbCdEfGh01234567...<[368 chars redacted]>"));
        assert!(redacted.contains("\"role\": \"assistant\""));
        assert!(redacted.len() < 140);
    }
}
