//! `web_search` tool wrapper around [`SearchProvider`].

use crate::ApprovalHandler;
use crate::approval::{begin_tool, finish_tool};
use crate::search::{SearchDepth, SearchOptions, SearchProvider};
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub content: String,
}

pub struct WebSearchTool {
    pub provider: Box<dyn SearchProvider>,
    pub approval: Arc<dyn ApprovalHandler>,
}

impl Tool for WebSearchTool {
    const NAME: &'static str = "web_search";

    type Error = std::io::Error;
    type Args = WebSearchArgs;
    type Output = WebSearchOutput;

    fn description(&self) -> String {
        "Search the web for up-to-date information, news, or documentation. Use this when the local context is insufficient. If the response contains a non-null `answer` field, treat it as a definitive answer and use it directly — do NOT call `http_fetch` to verify or re-fetch those URLs.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
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
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        begin_tool(self.approval.as_ref(), Self::NAME).await;

        let trimmed_query = args.query.trim();
        if trimmed_query.is_empty() {
            let out = WebSearchOutput {
                query: String::new(),
                answer: None,
                results: Vec::new(),
                error: Some("`query` must not be empty.".to_string()),
            };
            finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
            return Ok(out);
        }

        let depth = match args.search_depth.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("advanced") => SearchDepth::Advanced,
            _ => SearchDepth::Basic,
        };

        let options = SearchOptions {
            max_results: args.max_results.unwrap_or(5).clamp(1, 10),
            search_depth: depth,
            include_raw_content: false,
        };

        let out = match self.provider.search(trimmed_query, options).await {
            Ok(response) => WebSearchOutput {
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
                error: None,
            },
            Err(e) => WebSearchOutput {
                query: trimmed_query.to_string(),
                answer: None,
                results: Vec::new(),
                error: Some(format!("Search Error: {e}")),
            },
        };

        finish_tool(self.approval.as_ref(), Self::NAME, &out).await;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{SearchError, SearchResponse, SearchResult};
    use crate::test_support::MockApproval;

    struct SyntheticSearchProvider {
        fail: bool,
    }

    #[async_trait::async_trait]
    impl SearchProvider for SyntheticSearchProvider {
        fn name(&self) -> &'static str {
            "Synthetic"
        }

        async fn search(
            &self,
            query: &str,
            options: SearchOptions,
        ) -> Result<SearchResponse, SearchError> {
            if self.fail {
                return Err(SearchError::ApiError("synthetic 503".to_string()));
            }
            assert!((1..=10).contains(&options.max_results));
            Ok(SearchResponse {
                query: query.to_string(),
                answer: Some("Synthetic summary".to_string()),
                results: vec![SearchResult {
                    title: "Example Docs".to_string(),
                    url: "https://example.com/docs".to_string(),
                    content: "Snippet text".to_string(),
                    raw_content: None,
                    score: 0.95,
                }],
            })
        }
    }

    #[tokio::test]
    async fn web_search_validates_inputs_clamps_limits_and_reports_errors() {
        let approval = Arc::new(MockApproval::approved(None));
        let tool = WebSearchTool {
            provider: Box::new(SyntheticSearchProvider { fail: false }),
            approval: approval.clone(),
        };
        let mut ctx = ToolContext::default();

        let empty_q = tool
            .call(
                &mut ctx,
                WebSearchArgs {
                    query: "   ".to_string(),
                    search_depth: None,
                    max_results: None,
                },
            )
            .await
            .unwrap();
        assert!(empty_q.error.is_some());

        let ok = tool
            .call(
                &mut ctx,
                WebSearchArgs {
                    query: "rust async trait".to_string(),
                    search_depth: Some("ADVANCED".to_string()),
                    max_results: Some(0),
                },
            )
            .await
            .unwrap();
        assert!(ok.error.is_none());
        assert_eq!(ok.results.len(), 1);

        let failing_tool = WebSearchTool {
            provider: Box::new(SyntheticSearchProvider { fail: true }),
            approval: approval.clone(),
        };
        let err_out = failing_tool
            .call(
                &mut ctx,
                WebSearchArgs {
                    query: "rust".to_string(),
                    search_depth: None,
                    max_results: None,
                },
            )
            .await
            .unwrap();
        assert!(err_out.error.unwrap().contains("synthetic 503"));
        assert_eq!(approval.results.lock().len(), 3);
    }
}
