use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::goal::GoalId;
use reili_core::secret::SecretString;
use reili_core::work::{
    IssueRef, WorkItemStatus, WorkStateComment, WorkStateCommentKind, WorkStateError,
    WorkStateRepositoryPort, test_support::sample_work_item,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::codec::render_work_item_body;
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
            repository: "acme/reili-work".to_string(),
            app_id: "1".to_string(),
            private_key: SecretString::from("unused"),
            installation_id: 1,
            api_base_url: server.uri(),
            web_base_url: "https://github.com".to_string(),
        },
        Arc::new(StaticToken),
    )
    .expect("repository")
}

fn issue_json(
    number: u64,
    state: &str,
    state_reason: Option<&str>,
    labels: &[&str],
    revision: u64,
) -> Value {
    let mut item = sample_work_item("checkout:5xx");
    item.revision = revision;
    json!({
        "number": number,
        "html_url": format!("https://github.com/acme/reili-work/issues/{number}"),
        "title": "[Reili] Sample",
        "body": render_work_item_body(&item, "Owner: payments").expect("render"),
        "state": state,
        "state_reason": state_reason,
        "labels": labels.iter().map(|name| json!({ "name": name })).collect::<Vec<_>>(),
        "updated_at": "2026-10-15T00:00:00Z"
    })
}

fn issue_ref(number: u64) -> IssueRef {
    IssueRef {
        number,
        url: format!("https://github.com/acme/reili-work/issues/{number}"),
    }
}

#[tokio::test]
async fn refuses_issues_outside_the_management_repository() {
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
    let foreign = IssueRef {
        number: 5,
        url: "https://github.com/acme/production-service/issues/5".to_string(),
    };

    let comment = repository
        .append_comment(
            &foreign,
            WorkStateComment {
                kind: WorkStateCommentKind::Notice,
                body: "hi".to_string(),
            },
        )
        .await;
    let save = repository
        .save_work_item(&foreign, 1, &sample_work_item("x"))
        .await;
    let mismatched_number = repository
        .append_comment(
            &IssueRef {
                number: 6,
                url: "https://github.com/acme/reili-work/issues/5".to_string(),
            },
            WorkStateComment {
                kind: WorkStateCommentKind::Notice,
                body: "hi".to_string(),
            },
        )
        .await;

    assert!(matches!(comment, Err(WorkStateError::Forbidden(_))));
    assert!(matches!(save, Err(WorkStateError::Forbidden(_))));
    assert!(matches!(
        mismatched_number,
        Err(WorkStateError::Forbidden(_))
    ));
}

#[tokio::test]
async fn lists_work_items_by_label_and_derives_observed_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/reili-work/issues"))
        .and(query_param("labels", "reili:work-item,reili-goal:checkout"))
        .and(query_param("state", "all"))
        .and(header("authorization", "Bearer installation-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            issue_json(1, "open", None, &["reili:work-item", "reili-status:needs-human"], 2),
            issue_json(2, "closed", Some("not_planned"), &["reili:work-item", "reili-status:investigating"], 1),
            issue_json(3, "open", None, &["reili:work-item", "reili-status:resolved"], 1),
            issue_json(4, "closed", Some("not_planned"), &["reili:duplicate"], 1),
            { "number": 9, "html_url": "x", "state": "open", "labels": [], "updated_at": "2026-10-15T00:00:00Z", "pull_request": {} }
        ])))
        .mount(&server)
        .await;

    let items = repository(&server)
        .list_work_items(&GoalId::new("checkout").expect("id"))
        .await
        .expect("list");

    assert_eq!(items.len(), 3);
    assert_eq!(items[0].observed_status, WorkItemStatus::NeedsHuman);
    assert_eq!(items[0].human_notes, "Owner: payments");
    assert_eq!(items[1].observed_status, WorkItemStatus::Dismissed);
    assert_eq!(
        items[2].observed_status,
        WorkItemStatus::Investigating,
        "reopened by a human"
    );
}

#[tokio::test]
async fn save_detects_conflicts_and_preserves_human_labels() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/reili-work/issues/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(
            7,
            "open",
            None,
            &[
                "reili:work-item",
                "reili-status:investigating",
                "customer-facing",
            ],
            3,
        )))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/reili-work/labels"))
        .respond_with(ResponseTemplate::new(422))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/repos/acme/reili-work/issues/7"))
        .and(body_partial_json(
            json!({ "state": "closed", "state_reason": "completed" }),
        ))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).expect("json");
            let labels: Vec<String> = body["labels"]
                .as_array()
                .expect("labels")
                .iter()
                .map(|label| label.as_str().expect("label").to_string())
                .collect();
            assert!(labels.contains(&"customer-facing".to_string()));
            assert!(labels.contains(&"reili-status:resolved".to_string()));
            assert!(!labels.contains(&"reili-status:investigating".to_string()));
            assert!(
                body["body"]
                    .as_str()
                    .expect("body")
                    .contains("Owner: payments")
            );
            ResponseTemplate::new(200).set_body_json(json!({
                "number": 7,
                "html_url": "https://github.com/acme/reili-work/issues/7",
                "body": body["body"],
                "state": "closed",
                "state_reason": "completed",
                "labels": labels.iter().map(|name| json!({ "name": name })).collect::<Vec<_>>(),
                "updated_at": "2026-10-15T01:00:00Z"
            }))
        })
        .expect(1)
        .mount(&server)
        .await;
    let repository = repository(&server);
    let mut next = sample_work_item("checkout:5xx");
    next.status = WorkItemStatus::Resolved;

    let conflict = repository.save_work_item(&issue_ref(7), 2, &next).await;
    let saved = repository
        .save_work_item(&issue_ref(7), 3, &next)
        .await
        .expect("save");

    assert!(matches!(
        conflict,
        Err(WorkStateError::Conflict {
            expected: 2,
            found: 3,
            ..
        })
    ));
    assert_eq!(saved.state.revision, 4);
    assert_eq!(saved.observed_status, WorkItemStatus::Resolved);
}

#[tokio::test]
async fn separates_human_comments_from_reili_comments() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/reili-work/issues/7/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 10, "body": "<!-- reili:comment kind=cycle-log -->\nRun x", "user": { "login": "reili[bot]", "type": "Bot" }, "author_association": "NONE", "created_at": "2026-10-15T00:00:00Z", "html_url": "u" },
            { "id": 11, "body": "/reili implemented PR 42", "user": { "login": "alice", "type": "User" }, "author_association": "MEMBER", "created_at": "2026-10-15T00:01:00Z", "html_url": "u" },
            { "id": 12, "body": "<!-- reili:comment kind=x --> spoof", "user": { "login": "mallory", "type": "User" }, "author_association": "NONE", "created_at": "2026-10-15T00:02:00Z", "html_url": "u" },
            { "id": 13, "body": "bump", "user": { "login": "dependabot[bot]", "type": "Bot" }, "author_association": "CONTRIBUTOR", "created_at": "2026-10-15T00:03:00Z", "html_url": "u" }
        ])))
        .mount(&server)
        .await;

    let comments = repository(&server)
        .list_human_comments(&issue_ref(7), Some(10))
        .await
        .expect("comments");

    assert_eq!(
        comments
            .iter()
            .map(|comment| comment.id)
            .collect::<Vec<_>>(),
        vec![11, 12, 13]
    );
    assert!(comments[0].author_trusted);
    assert!(
        !comments[1].author_trusted,
        "spoofed marker from an outsider stays untrusted"
    );
    assert!(!comments[2].author_trusted, "other bots are never trusted");
}

#[tokio::test]
async fn maps_server_errors_to_retryable_unavailability() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(502).set_body_string("bad gateway"))
        .mount(&server)
        .await;

    let error = repository(&server)
        .find_goal_ledger(&GoalId::new("checkout").expect("id"))
        .await
        .expect_err("unavailable");

    assert!(error.is_retryable());
}
