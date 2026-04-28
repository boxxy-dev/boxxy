use crate::search::{SearchDepth, SearchOptions, SearchProvider};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct WebSearchArgs {
    pub query: String,
    pub search_depth: Option<String>,
    pub max_results: Option<usize>,
}

#[derive(Serialize)]
pub struct WebSearchOutput {
    pub query: String,
    pub answer: Option<String>,
    pub results: Vec<WebSearchResult>,
}

#[derive(Serialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub content: String,
}

pub struct WebSearchTool {
    pub provider: Box<dyn SearchProvider>,
    pub approval: std::sync::Arc<dyn crate::ApprovalHandler>,
}

impl Tool for WebSearchTool {
    const NAME: &'static str = "web_search";

    type Error = std::io::Error;
    type Args = WebSearchArgs;
    type Output = WebSearchOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search the web for up-to-date information, news, or documentation. Use this when the local context is insufficient. If the response contains a non-null `answer` field, treat it as a definitive answer and use it directly — do NOT call `http_fetch` to verify or re-fetch those URLs.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query to execute."
                    },
                    "search_depth": {
                        "type": "string",
                        "enum": ["basic", "advanced"],
                        "description": "The depth of the search. 'advanced' is slower but more thorough. Defaults to 'basic'."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "The maximum number of results to return (1-10). Defaults to 5."
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval
            .report_tool_started(Self::NAME.to_string())
            .await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;

        let depth = match args.search_depth.as_deref() {
            Some("advanced") => SearchDepth::Advanced,
            _ => SearchDepth::Basic,
        };

        let options = SearchOptions {
            max_results: args.max_results.unwrap_or(5).min(10),
            search_depth: depth,
            include_raw_content: false, // We keep it simple for now to save tokens
        };

        match self.provider.search(&args.query, options).await {
            Ok(response) => {
                let out = WebSearchOutput {
                    query: response.query,
                    answer: response.answer,
                    results: response
                        .results
                        .into_iter()
                        .map(|r| WebSearchResult {
                            title: r.title,
                            url: r.url,
                            content: r.content,
                        })
                        .collect(),
                };
                self.approval
                    .report_tool_result(
                        Self::NAME.to_string(),
                        serde_json::to_string(&out).unwrap_or_default(),
                    )
                    .await;
                Ok(out)
            }
            Err(e) => Err(std::io::Error::other(format!("Search Error: {e}"))),
        }
    }
}
