use crate::search::{
    SearchDepth, SearchError, SearchOptions, SearchProvider, SearchResponse, SearchResult,
};
use serde::{Deserialize, Serialize};

pub struct TavilyProvider {
    api_key: String,
    client: reqwest::Client,
}

impl TavilyProvider {
    pub fn new(api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default();

        Self { api_key, client }
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

#[async_trait::async_trait]
impl SearchProvider for TavilyProvider {
    async fn search(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<SearchResponse, SearchError> {
        let depth = match options.search_depth {
            SearchDepth::Basic => "basic",
            SearchDepth::Advanced => "advanced",
        };

        let request = TavilyRequest {
            api_key: &self.api_key,
            query,
            search_depth: depth,
            max_results: options.max_results.max(1).min(20),
            include_raw_content: options.include_raw_content,
            include_answer: true, // We always want the LLM-generated answer if available
        };

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .json(&request)
            .send()
            .await
            .map_err(|e| SearchError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown API error".to_string());
            return Err(SearchError::ApiError(error_text));
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
