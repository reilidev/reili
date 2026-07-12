use async_trait::async_trait;

use crate::error::PortError;

/// Downloads the raw bytes of a Slack-hosted file (e.g. `url_private_download`).
///
/// Implementations authenticate with the Slack bot token so private files can be fetched.
/// `max_bytes` caps the download so oversized files are rejected instead of buffered in memory.
#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackFileDownloadPort: Send + Sync {
    async fn download_file(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, PortError>;
}
