use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::SlackUserGroupMembershipPort;
use serde::{Deserialize, Serialize};

use super::slack_web_api_client::SlackWebApiClient;

#[derive(Debug, Clone)]
pub struct SlackUserGroupMembershipAdapter {
    client: Arc<SlackWebApiClient>,
}

#[derive(Debug, Serialize)]
struct UserGroupsUsersListQuery {
    usergroup: String,
}

#[derive(Debug, Deserialize)]
struct UserGroupsUsersListResponse {
    users: Vec<String>,
}

impl SlackUserGroupMembershipAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SlackUserGroupMembershipPort for SlackUserGroupMembershipAdapter {
    async fn list_user_group_members(&self, user_group_id: &str) -> Result<Vec<String>, PortError> {
        let user_group_id = user_group_id.trim();
        if user_group_id.is_empty() {
            return Err(PortError::invalid_input(
                "Slack user group ID must not be empty",
            ));
        }

        let query = UserGroupsUsersListQuery {
            usergroup: user_group_id.to_string(),
        };

        let response = self.client.get("usergroups.users.list", &query).await?;

        let parsed: UserGroupsUsersListResponse =
            serde_json::from_value(response).map_err(|error| {
                PortError::invalid_response(format!(
                    "Failed to parse Slack usergroups.users.list response JSON: {error}"
                ))
            })?;

        Ok(parsed
            .users
            .into_iter()
            .map(|user| user.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::messaging::slack::SlackUserGroupMembershipPort;
    use reili_core::secret::SecretString;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackUserGroupMembershipAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn reads_member_user_ids_from_usergroups_users_list_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/usergroups.users.list"))
            .and(query_param("usergroup", "S123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "users": ["U001", "U002", ""]
            })))
            .mount(&server)
            .await;

        let adapter = SlackUserGroupMembershipAdapter::new(Arc::new(create_client(&server.uri())));
        let members = adapter
            .list_user_group_members("S123")
            .await
            .expect("list user group members");

        assert_eq!(members, vec!["U001".to_string(), "U002".to_string()]);
    }

    #[tokio::test]
    async fn rejects_response_when_users_field_is_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/usergroups.users.list"))
            .and(query_param("usergroup", "S123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true
            })))
            .mount(&server)
            .await;

        let adapter = SlackUserGroupMembershipAdapter::new(Arc::new(create_client(&server.uri())));
        let error = adapter
            .list_user_group_members("S123")
            .await
            .expect_err("missing users should fail");

        assert!(
            error
                .message
                .contains("Slack usergroups.users.list response JSON")
        );
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: SecretString::from("xoxb-test"),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
