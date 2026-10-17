use crate::ApprovalHandler;
use rig::tool::PortableTool;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock};

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("Boxxy-Claw/0.1.0")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("Failed to initialize shared HTTP client")
});

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

impl PortableTool for HttpFetchTool {
    const NAME: &'static str = "http_fetch";

    type Error = std::io::Error;
    type Args = HttpFetchArgs;
    type Output = HttpFetchOutput;

    fn description(&self) -> String {
        "Fetch the full text content of a URL. Use this ONLY when: (a) the user provides a specific URL to read, or (b) `web_search` returned no useful answer and you must read a specific page in full. Do NOT use this to re-fetch or verify URLs already returned by `web_search` — those results are already summarised. Returns the status code, response body, and headers.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
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
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let client = &*HTTP_CLIENT;

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

fn is_skipped_tag(tag_name: &str) -> bool {
    matches!(
        tag_name,
        "script" | "style" | "svg" | "noscript" | "nav" | "footer" | "head"
    )
}

pub(crate) fn strip_html_tags(html: &str) -> String {
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
                let t = tag_buf.trim().to_ascii_lowercase();
                let is_closing = t.starts_with('/');
                let is_self_closing = t.ends_with('/');
                let name = t
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or("");

                if !t.starts_with('!') {
                    if !is_closing && !is_self_closing && is_skipped_tag(name) {
                        skip_depth += 1;
                    } else if is_closing && is_skipped_tag(name) {
                        skip_depth = (skip_depth - 1).max(0);
                    } else if skip_depth == 0 {
                        match (is_closing, name) {
                            (false, "h1") => {
                                out.push_str("\n# ");
                                last_was_space = true;
                            }
                            (false, "h2") => {
                                out.push_str("\n## ");
                                last_was_space = true;
                            }
                            (false, "h3") => {
                                out.push_str("\n### ");
                                last_was_space = true;
                            }
                            (false, "h4" | "h5" | "h6") => {
                                out.push_str("\n#### ");
                                last_was_space = true;
                            }
                            (false, "li") => {
                                out.push_str("\n- ");
                                last_was_space = true;
                            }
                            (false, "pre") => {
                                out.push_str("\n```\n");
                                last_was_space = true;
                            }
                            (true, "pre") => {
                                out.push_str("\n```\n");
                                last_was_space = true;
                            }
                            (_, "code") => {
                                out.push('`');
                            }
                            (_, "p" | "div" | "section" | "article" | "tr" | "br" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li") => {
                                if !out.ends_with('\n') {
                                    out.push('\n');
                                }
                                last_was_space = true;
                            }
                            _ => {}
                        }
                    }
                }
                tag_buf.clear();
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

    // Decode common HTML entities and collapse excessive blank lines
    let decoded = out
        .trim()
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    let mut clean_lines = Vec::new();
    let mut prev_empty = false;
    for line in decoded.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_empty {
                clean_lines.push("");
                prev_empty = true;
            }
        } else {
            clean_lines.push(trimmed);
            prev_empty = false;
        }
    }

    clean_lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_tags_removes_noise_and_preserves_markdown_structure() {
        let html = r#"
            <html>
            <head><title>Ignored Title</title><style>.a { color: red; }</style></head>
            <body>
                <nav><a href="/">Home</a></nav>
                <svg><path d="M0 0"/></svg>
                <h1>Main Heading</h1>
                <p>Use <code>cargo test</code> to run tests:</p>
                <ul>
                    <li>First item</li>
                    <li>Second item</li>
                </ul>
                <pre>fn main() {}</pre>
                <footer>Copyright 2026</footer>
                <script>console.log("ignore me");</script>
            </body>
            </html>
        "#;

        let text = strip_html_tags(html);
        assert!(!text.contains("Ignored Title"));
        assert!(!text.contains("color: red"));
        assert!(!text.contains("Home"));
        assert!(!text.contains("Copyright"));
        assert!(!text.contains("console.log"));
        assert!(text.contains("# Main Heading"));
        assert!(text.contains("`cargo test`"));
        assert!(text.contains("- First item"));
        assert!(text.contains("```\nfn main() {}\n```"));
    }
}
