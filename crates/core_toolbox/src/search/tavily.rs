//! Tavily web search provider backed by a shared HTTP connection pool.

use crate::search::{
    SearchDepth, SearchError, SearchOptions, SearchProvider, SearchResponse, SearchResult,
};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use std::time::Duration;

const MAX_ERROR_BODY_CHARS: usize = 512;

static TAVILY_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(concat!("Boxxy-Claw/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .expect("Failed to initialize shared Tavily HTTP client")
});

pub struct TavilyProvider {
    api_key: String,
}

impl TavilyProvider {
    pub const NAME: &'static str = "Tavily";

    #[must_use]
    pub const fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

#[derive(Serialize)]
struct TavilyRequest<'a> {
    api_key: &'a str,
    query: &'a str,
    search_depth: &'static str,
    max_results: usize,
    include_raw_content: bool,
    include_answer: bool,
}

#[derive(Deserialize)]
struct TavilyResponse {
    query: String,
    results: Vec<TavilyResult>,
    answer: Option<String>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
    raw_content: Option<String>,
    score: f64,
}

fn clip_error_body(status: reqwest::StatusCode, raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return format!("HTTP {status}");
    }
    let cut = trimmed.floor_char_boundary(trimmed.len().min(MAX_ERROR_BODY_CHARS));
    if cut < trimmed.len() {
        format!("HTTP {status}: {}…", &trimmed[..cut])
    } else {
        format!("HTTP {status}: {trimmed}")
    }
}

#[async_trait::async_trait]
impl SearchProvider for TavilyProvider {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    async fn search(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<SearchResponse, SearchError> {
        if self.api_key.trim().is_empty() {
            return Err(SearchError::ConfigError(
                "Tavily API key is empty.".to_string(),
            ));
        }

        let depth = match options.search_depth {
            SearchDepth::Basic => "basic",
            SearchDepth::Advanced => "advanced",
        };

        let request = TavilyRequest {
            api_key: &self.api_key,
            query,
            search_depth: depth,
            max_results: options.max_results.clamp(1, 20),
            include_raw_content: options.include_raw_content,
            include_answer: true,
        };

        let response = TAVILY_CLIENT
            .post("https://api.tavily.com/search")
            .json(&request)
            .send()
            .await
            .map_err(|e| SearchError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(SearchError::ApiError(clip_error_body(status, &error_text)));
        }

        let tavily_res: TavilyResponse = response
            .json()
            .await
            .map_err(|e| SearchError::ApiError(format!("Failed to parse JSON: {e}")))?;

        Ok(SearchResponse {
            query: tavily_res.query,
            answer: tavily_res.answer,
            results: tavily_res
                .results
                .into_iter()
                .map(|r| SearchResult {
                    title: r.title,
                    url: r.url,
                    content: r.content,
                    raw_content: r.raw_content,
                    score: r.score,
                })
                .collect(),
        })
    }
}
