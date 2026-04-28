use crate::ApprovalHandler;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct HttpFetchArgs {
    pub url: String,
    pub method: Option<String>,
}

#[derive(Serialize)]
pub struct HttpFetchOutput {
    pub status: u16,
    pub body: String,
    pub headers: std::collections::HashMap<String, String>,
}

pub struct HttpFetchTool {
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for HttpFetchTool {
    const NAME: &'static str = "http_fetch";

    type Error = std::io::Error;
    type Args = HttpFetchArgs;
    type Output = HttpFetchOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Fetch the full text content of a URL. Use this ONLY when: (a) the user provides a specific URL to read, or (b) `web_search` returned no useful answer and you must read a specific page in full. Do NOT use this to re-fetch or verify URLs already returned by `web_search` — those results are already summarised. Returns the status code, response body, and headers.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The absolute URL to fetch (must start with http:// or https://)."
                    },
                    "method": {
                        "type": "string",
                        "description": "The HTTP method to use (e.g. GET, POST). Defaults to GET."
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let client = reqwest::Client::builder()
            .user_agent("Boxxy-Claw/0.1.0")
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| std::io::Error::other(format!("Failed to build HTTP client: {e}")))?;

        let method = match args
            .method
            .as_deref()
            .unwrap_or("GET")
            .to_uppercase()
            .as_str()
        {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            "PATCH" => reqwest::Method::PATCH,
            "HEAD" => reqwest::Method::HEAD,
            _ => reqwest::Method::GET,
        };

        let mut response = client
            .request(method, &args.url)
            .send()
            .await
            .map_err(|e| std::io::Error::other(format!("HTTP Request failed: {e}")))?;

        let status = response.status().as_u16();
        let mut headers = std::collections::HashMap::new();
        for (name, value) in response.headers().iter() {
            headers.insert(name.to_string(), value.to_str().unwrap_or("").to_string());
        }

        let is_html = headers
            .get("content-type")
            .map(|ct| ct.contains("text/html"))
            .unwrap_or(false);

        // HTML pages: cap raw read at 200KB then strip tags (yields ~20–50KB text).
        // Everything else: cap at 1MB as before.
        let max_bytes = if is_html { 200 * 1024 } else { 1024 * 1024 };
        let mut was_truncated = false;
        let mut body_bytes = Vec::new();
        while let Ok(Some(chunk)) = response.chunk().await {
            if body_bytes.len() + chunk.len() > max_bytes {
                body_bytes.extend_from_slice(&chunk[..max_bytes - body_bytes.len()]);
                was_truncated = true;
                break;
            }
            body_bytes.extend_from_slice(&chunk);
        }

        let body = if is_html {
            const MAX_TEXT: usize = 50_000;
            let raw = String::from_utf8_lossy(&body_bytes);
            let text = strip_html_tags(&raw);
            if text.len() > MAX_TEXT {
                format!(
                    "{}\n\n[NOTE: Page text truncated to 50KB]",
                    &text[..MAX_TEXT]
                )
            } else if was_truncated {
                format!(
                    "{}\n\n[NOTE: Source HTML was truncated before stripping]",
                    text
                )
            } else {
                text
            }
        } else {
            let mut s = String::from_utf8_lossy(&body_bytes).to_string();
            if was_truncated {
                s.push_str("\n\n[WARNING: Response body truncated at 1MB]");
            }
            s
        };

        let out = HttpFetchOutput {
            status,
            body,
            headers,
        };
        self.approval
            .report_tool_result(
                Self::NAME.to_string(),
                serde_json::to_string(&out).unwrap_or_default(),
            )
            .await;
        Ok(out)
    }
}

fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 4);
    let mut in_tag = false;
    let mut skip_depth: i32 = 0;
    let mut tag_buf = String::new();
    let mut last_was_space = true;

    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag_buf.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let t = tag_buf.trim().to_lowercase();
                if t.starts_with("script") || t.starts_with("style") {
                    skip_depth += 1;
                } else if t.starts_with("/script") || t.starts_with("/style") {
                    skip_depth = (skip_depth - 1).max(0);
                }
                tag_buf.clear();
                if skip_depth == 0 && !last_was_space {
                    out.push('\n');
                    last_was_space = true;
                }
            }
            _ if in_tag => tag_buf.push(ch),
            _ if skip_depth > 0 => {}
            ch if ch.is_whitespace() => {
                if !last_was_space {
                    out.push(' ');
                    last_was_space = true;
                }
            }
            _ => {
                out.push(ch);
                last_was_space = false;
            }
        }
    }

    // Decode the most common HTML entities
    out.trim()
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}
