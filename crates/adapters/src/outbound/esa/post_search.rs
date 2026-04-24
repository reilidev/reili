use async_trait::async_trait;
use reili_core::error::PortError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EsaPostSearchInput {
    pub q: String,
    pub page: u32,
    pub per_page: u32,
    pub sort: EsaPostSearchSort,
    pub order: EsaPostSearchOrder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EsaPostSearchSort {
    Updated,
    Created,
    Number,
    Stars,
    Watches,
    Comments,
    BestMatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EsaPostSearchOrder {
    Desc,
    Asc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EsaPostSearchResult {
    pub posts: Vec<EsaPost>,
    pub prev_page: Option<u32>,
    pub next_page: Option<u32>,
    pub total_count: u32,
    pub page: u32,
    pub per_page: u32,
    pub max_per_page: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EsaPost {
    pub number: u64,
    pub name: String,
    pub wip: bool,
    pub body_md: String,
    pub url: Option<String>,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub created_by: Option<EsaUser>,
    pub updated_by: Option<EsaUser>,
    pub comments_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EsaUser {
    pub name: Option<String>,
    pub screen_name: Option<String>,
}

#[async_trait]
pub trait EsaPostSearchPort: Send + Sync {
    async fn search_posts(
        &self,
        input: EsaPostSearchInput,
    ) -> Result<EsaPostSearchResult, PortError>;
}
