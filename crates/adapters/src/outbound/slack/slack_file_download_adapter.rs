use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::SlackFileDownloadPort;

use super::slack_web_api_client::SlackWebApiClient;

#[derive(Debug, Clone)]
pub struct SlackFileDownloadAdapter {
    client: Arc<SlackWebApiClient>,
}

impl SlackFileDownloadAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SlackFileDownloadPort for SlackFileDownloadAdapter {
    async fn download_file(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, PortError> {
        self.client.download_bytes(url, max_bytes).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::messaging::slack::SlackFileDownloadPort;
    use reili_core::secret::SecretString;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackFileDownloadAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn downloads_file_via_web_api_client() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/files/incident.pdf"))
            .and(header("Authorization", "Bearer xoxb-test"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_bytes(b"%PDF-1.4".to_vec()),
            )
            .mount(&server)
            .await;

        let adapter = SlackFileDownloadAdapter::new(Arc::new(create_client(&server.uri())));
        let bytes = adapter
            .download_file(
                &format!("{}/files/incident.pdf", server.uri()),
                32 * 1024 * 1024,
            )
            .await
            .expect("download file");

        assert_eq!(bytes, b"%PDF-1.4".to_vec());
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: SecretString::from("xoxb-test"),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
