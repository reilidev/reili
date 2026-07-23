use async_trait::async_trait;
use reili_core::error::PortError;
use serde::{Deserialize, Serialize};

use super::EsaPost;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EsaPostGetInput {
    pub number: u64,
}

#[async_trait]
pub trait EsaPostGetPort: Send + Sync {
    async fn get_post(&self, input: EsaPostGetInput) -> Result<EsaPost, PortError>;
}
