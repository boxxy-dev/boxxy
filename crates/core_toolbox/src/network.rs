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
            description: "Fetch content from a URL. Use this to read documentation, check API statuses, or download data from the web. Returns the status code, response body, and headers.".to_string(),
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
        self.approval.report_tool_started(Self::NAME.to_string()).await;
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

        // Limit response size to 1MB to avoid context flooding
        const MAX_SIZE: usize = 1024 * 1024;
        let mut body_bytes = Vec::new();
        while let Ok(Some(chunk)) = response.chunk().await {
            if body_bytes.len() + chunk.len() > MAX_SIZE {
                body_bytes.extend_from_slice(&chunk[..MAX_SIZE - body_bytes.len()]);
                let body = String::from_utf8_lossy(&body_bytes).to_string();
                let out = HttpFetchOutput {
                    status,
                    body: format!("{body}\n\n[WARNING: Response body truncated at 1MB]"),
                    headers,
                };
                self.approval
                    .report_tool_result(
                        Self::NAME.to_string(),
                        serde_json::to_string(&out).unwrap_or_default(),
                    )
                    .await;
                return Ok(out);
            }
            body_bytes.extend_from_slice(&chunk);
        }

        let body = String::from_utf8_lossy(&body_bytes).to_string();

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
