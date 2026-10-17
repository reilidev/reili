//! Stateful stand-in for the GitHub Issues REST API, so the real adapter can be run against the
//! backend contract over HTTP.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use reili_core::work::{RecordRef, WorkItemStatus, WorkStateRepositoryPort};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::any};

use super::repository::GitHubWorkStateRepository;

const REPOSITORY_PATH: &str = "/repos/acme/reili-work";
const WEB_URL: &str = "https://github.com/acme/reili-work";

#[derive(Default)]
struct FakeState {
    issues: Vec<Value>,
    comments: Vec<(u64, Value)>,
    next_comment_id: u64,
    clock: i64,
}

impl FakeState {
    fn tick(&mut self) -> String {
        self.clock += 1;
        chrono::DateTime::from_timestamp(1_760_000_000 + self.clock, 0)
            .unwrap_or_default()
            .to_rfc3339()
    }

    fn issue_mut(&mut self, number: u64) -> Option<&mut Value> {
        self.issues
            .iter_mut()
            .find(|issue| issue["number"].as_u64() == Some(number))
    }

    fn add_comment(&mut self, number: u64, body: &str, login: &str, kind: &str, association: &str) {
        self.next_comment_id += 1;
        let id = self.next_comment_id;
        let created_at = self.tick();
        self.comments.push((
            number,
            json!({
                "id": id,
                "body": body,
                "user": { "login": login, "type": kind },
                "author_association": association,
                "created_at": created_at,
                "html_url": format!("{WEB_URL}/issues/{number}#issuecomment-{id}"),
            }),
        ));
    }
}

#[derive(Clone, Default)]
pub(super) struct FakeGitHub {
    state: Arc<Mutex<FakeState>>,
}

impl FakeGitHub {
    pub(super) async fn mount(&self, server: &MockServer) {
        Mock::given(any())
            .respond_with(self.clone())
            .mount(server)
            .await;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn list_issues(state: &FakeState, request: &Request) -> ResponseTemplate {
        let query = |name: &str| {
            request
                .url
                .query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        let labels: Vec<String> = query("labels")
            .unwrap_or_default()
            .split(',')
            .filter(|label| !label.is_empty())
            .map(str::to_string)
            .collect();
        let per_page: usize = query("per_page").and_then(|v| v.parse().ok()).unwrap_or(30);
        let page: usize = query("page").and_then(|v| v.parse().ok()).unwrap_or(1);
        let matching: Vec<&Value> = state
            .issues
            .iter()
            .filter(|issue| {
                labels.iter().all(|wanted| {
                    issue["labels"]
                        .as_array()
                        .is_some_and(|names| names.iter().any(|label| label["name"] == **wanted))
                })
            })
            .collect();
        let page_items: Vec<&Value> = matching
            .into_iter()
            .skip((page - 1) * per_page)
            .take(per_page)
            .collect();
        ResponseTemplate::new(200).set_body_json(page_items)
    }

    fn create_issue(state: &mut FakeState, payload: &Value) -> ResponseTemplate {
        let number = state.issues.len() as u64 + 1;
        let updated_at = state.tick();
        let issue = json!({
            "number": number,
            "html_url": format!("{WEB_URL}/issues/{number}"),
            "title": payload["title"],
            "body": payload["body"],
            "state": "open",
            "state_reason": null,
            "labels": label_objects(&payload["labels"]),
            "updated_at": updated_at,
        });
        state.issues.push(issue.clone());
        ResponseTemplate::new(201).set_body_json(issue)
    }

    fn update_issue(state: &mut FakeState, number: u64, payload: &Value) -> ResponseTemplate {
        let updated_at = state.tick();
        let Some(issue) = state.issue_mut(number) else {
            return ResponseTemplate::new(404);
        };
        for field in ["title", "body", "state", "state_reason"] {
            if let Some(value) = payload.get(field) {
                issue[field] = value.clone();
            }
        }
        if let Some(labels) = payload.get("labels") {
            issue["labels"] = label_objects(labels);
        }
        issue["updated_at"] = json!(updated_at);
        ResponseTemplate::new(200).set_body_json(issue.clone())
    }

    fn list_comments(state: &FakeState, number: u64, request: &Request) -> ResponseTemplate {
        let page = request
            .url
            .query_pairs()
            .find(|(key, _)| key == "page")
            .and_then(|(_, value)| value.parse::<usize>().ok())
            .unwrap_or(1);
        let comments: Vec<&Value> = state
            .comments
            .iter()
            .filter(|(issue, _)| *issue == number)
            .map(|(_, comment)| comment)
            .filter(|_| page == 1)
            .collect();
        ResponseTemplate::new(200).set_body_json(comments)
    }
}

fn label_objects(names: &Value) -> Value {
    Value::Array(
        names
            .as_array()
            .into_iter()
            .flatten()
            .map(|name| json!({ "name": name }))
            .collect(),
    )
}

impl Respond for FakeGitHub {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.lock();
        let Some(route) = request.url.path().strip_prefix(REPOSITORY_PATH) else {
            return ResponseTemplate::new(404);
        };
        let payload: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let segments: Vec<&str> = route.trim_start_matches('/').split('/').collect();
        let number = segments.get(1).and_then(|value| value.parse::<u64>().ok());
        match (request.method.as_str(), segments.as_slice(), number) {
            ("GET", ["issues"], _) => Self::list_issues(&state, request),
            ("POST", ["issues"], _) => Self::create_issue(&mut state, &payload),
            ("GET", ["issues", _], Some(number)) => state
                .issue_mut(number)
                .map_or(ResponseTemplate::new(404), |issue| {
                    ResponseTemplate::new(200).set_body_json(issue.clone())
                }),
            ("PATCH", ["issues", _], Some(number)) => {
                Self::update_issue(&mut state, number, &payload)
            }
            ("GET", ["issues", _, "comments"], Some(number)) => {
                Self::list_comments(&state, number, request)
            }
            ("POST", ["issues", _, "comments"], Some(number)) => {
                let body = payload["body"].as_str().unwrap_or_default().to_string();
                state.add_comment(number, &body, "reili[bot]", "Bot", "NONE");
                ResponseTemplate::new(201).set_body_json(json!({}))
            }
            ("POST", ["labels"], _) => ResponseTemplate::new(201).set_body_json(json!({})),
            _ => ResponseTemplate::new(404),
        }
    }
}

pub(super) struct GitHubContractHarness {
    pub(super) fake: FakeGitHub,
    pub(super) repository: Arc<GitHubWorkStateRepository>,
}

fn issue_number(record: &RecordRef) -> u64 {
    record
        .key
        .as_str()
        .trim_start_matches('#')
        .parse()
        .unwrap_or_default()
}

#[async_trait]
impl reili_core::work::conformance::WorkStateBackendHarness for GitHubContractHarness {
    fn repository(&self) -> Arc<dyn WorkStateRepositoryPort> {
        self.repository.clone()
    }

    async fn comment_as_human(&self, record: &RecordRef, author: &str, trusted: bool, body: &str) {
        let association = if trusted { "MEMBER" } else { "NONE" };
        self.fake
            .lock()
            .add_comment(issue_number(record), body, author, "User", association);
    }

    async fn set_status_as_human(&self, record: &RecordRef, status: WorkItemStatus) {
        let mut state = self.fake.lock();
        let updated_at = state.tick();
        let Some(issue) = state.issue_mut(issue_number(record)) else {
            return;
        };
        let mut labels: Vec<Value> = issue["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|label| {
                !label["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("reili-status:"))
            })
            .cloned()
            .collect();
        labels.push(json!({ "name": format!("reili-status:{}", status.as_str()) }));
        issue["labels"] = Value::Array(labels);
        issue["state"] = json!(if status.is_open() { "open" } else { "closed" });
        issue["updated_at"] = json!(updated_at);
    }
}
