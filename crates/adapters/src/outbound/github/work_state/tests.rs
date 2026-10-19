use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use reili_core::error::PortError;
use reili_core::objective::ObjectiveId;
use reili_core::secret::SecretString;
use reili_core::work::{RecordKey, RecordRef, WorkStateError, WorkStatePort};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::fake_github::REILI_APP_ID;
use super::repository::{AccessTokenSource, GitHubWorkStateConfig, GitHubWorkStateRepository};

struct StaticToken;

#[async_trait]
impl AccessTokenSource for StaticToken {
    async fn token(&self) -> Result<SecretString, PortError> {
        Ok(SecretString::from("installation-token"))
    }
}

fn repository(server: &MockServer) -> GitHubWorkStateRepository {
    GitHubWorkStateRepository::with_token_source(
        &GitHubWorkStateConfig {
            repository: "acme/reili-work".parse().expect("repository"),
            app_id: REILI_APP_ID.to_string(),
            private_key: SecretString::from("unused"),
            installation_id: 1,
            api_base_url: server.uri(),
            web_base_url: "https://github.com".to_string(),
        },
        Arc::new(StaticToken),
    )
    .expect("repository")
}

fn record(key: &str, url: &str) -> RecordRef {
    RecordRef {
        key: RecordKey::new(key),
        url: url.to_string(),
    }
}

#[tokio::test]
async fn refuses_records_outside_the_configured_repository() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let repository = repository(&server);
    let foreign = record("#5", "https://github.com/acme/production-service/issues/5");

    let comment = repository.add_comment(&foreign, "hi").await;
    let close = repository.close_work_item(&foreign).await;
    let mismatched_number = repository
        .add_comment(
            &record("#6", "https://github.com/acme/reili-work/issues/5"),
            "hi",
        )
        .await;
    let foreign_key = repository
        .add_comment(
            &record("OPS-5", "https://github.com/acme/reili-work/issues/5"),
            "hi",
        )
        .await;

    for result in [comment, close, mismatched_number, foreign_key] {
        assert!(matches!(result, Err(WorkStateError::Forbidden(_))));
    }
}

#[tokio::test]
async fn recognises_reilis_issues_and_comments_by_its_app_id() {
    let server = MockServer::start().await;
    let issue = |number: u64, app_id: u64| {
        json!({
            "number": number,
            "html_url": format!("https://github.com/acme/reili-work/issues/{number}"),
            "title": "5xx after deploy",
            "body": "Text",
            "user": { "login": "some-app[bot]", "type": "Bot" },
            "state": "open",
            "labels": [{ "name": "reili-objective:checkout" }, { "name": "reili:assignee" }],
            "created_at": "2026-10-15T00:00:00Z",
            "updated_at": "2026-10-15T00:00:00Z",
            "performed_via_github_app": { "id": app_id }
        })
    };
    let comment = |app_id: u64| {
        json!({
            "body": "Progress",
            "user": { "login": "some-app[bot]", "type": "Bot" },
            "author_association": "NONE",
            "created_at": "2026-10-15T00:00:00Z",
            "html_url": "https://github.com/acme/reili-work/issues/1#issuecomment-1",
            "performed_via_github_app": { "id": app_id }
        })
    };
    Mock::given(method("GET"))
        .and(path("/repos/acme/reili-work/issues"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            issue(1, REILI_APP_ID),
            issue(2, 99),
            { "number": 3, "html_url": "x", "state": "open", "labels": [], "created_at": "2026-10-15T00:00:00Z", "updated_at": "2026-10-15T00:00:00Z", "pull_request": {} }
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/reili-work/issues"))
        .and(query_param("state", "closed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/reili-work/issues/1/comments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([comment(REILI_APP_ID), comment(99)])),
        )
        .mount(&server)
        .await;
    let repository = repository(&server);

    let items = repository
        .list_work_items(
            &ObjectiveId::new("checkout").expect("id"),
            Utc.timestamp_opt(0, 0).single().expect("time"),
        )
        .await
        .expect("list");
    let comments = repository
        .list_comments(&items[0].record)
        .await
        .expect("comments");

    assert_eq!(items.len(), 2, "pull requests are not work items");
    assert!(items[0].created_by_reili);
    assert!(
        !items[1].created_by_reili,
        "issues from other Apps are not Reili's"
    );
    assert!(comments[0].by_reili);
    assert!(
        !comments[1].by_reili,
        "comments from other Apps are not Reili's"
    );
}

#[tokio::test]
async fn maps_server_errors_to_retryable_unavailability() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(502).set_body_string("bad gateway"))
        .mount(&server)
        .await;

    let error = repository(&server)
        .list_work_items(
            &ObjectiveId::new("checkout").expect("id"),
            Utc.timestamp_opt(0, 0).single().expect("time"),
        )
        .await
        .expect_err("unavailable");

    assert!(matches!(error, WorkStateError::Unavailable(_)));
}

#[tokio::test]
async fn github_backend_satisfies_the_work_state_contract() {
    let server = MockServer::start().await;
    let fake = super::fake_github::FakeGitHub::default();
    fake.mount(&server).await;
    let harness = super::fake_github::GitHubContractHarness {
        fake,
        repository: Arc::new(repository(&server)),
    };

    reili_core::work::conformance::assert_work_state_contract(&harness).await;
}
