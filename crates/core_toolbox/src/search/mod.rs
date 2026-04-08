use serde::{Deserialize, Serialize};

#[async_trait::async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Result<SearchResponse, SearchError>;
}

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub max_results: usize,
    pub search_depth: SearchDepth,
    pub include_raw_content: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum SearchDepth {
    #[default]
    Basic,
    Advanced,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub answer: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub content: String,
    pub raw_content: Option<String>,
    pub score: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("API Error: {0}")]
    ApiError(String),
    #[error("Network Error: {0}")]
    NetworkError(String),
    #[error("Config Error: {0}")]
    ConfigError(String),
}

pub mod tavily;
pub mod tool;

pub use tavily::TavilyProvider;
pub use tool::WebSearchTool;
