use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use reili_core::objective::ObjectiveId;
use reili_core::work::{RecordKey, RecordRef, WorkStatePort};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::any};

use super::repository::{ASSIGNEE_LABEL, GitHubWorkStateRepository};

const REPOSITORY_PATH: &str = "/repos/acme/reili-work";
const WEB_URL: &str = "https://github.com/acme/reili-work";
const BOT: (&str, &str) = ("reili[bot]", "Bot");
/// Matches the App ID the adapter under test is configured with.
pub(super) const REILI_APP_ID: u64 = 1;

fn performed_via(user: (&str, &str)) -> Value {
    if user == BOT {
        json!({ "id": REILI_APP_ID })
    } else {
        Value::Null
    }
}

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

    fn add_issue(&mut self, title: &str, body: &str, labels: &Value, user: (&str, &str)) -> Value {
        let number = self.issues.len() as u64 + 1;
        let now = self.tick();
        let issue = json!({
            "number": number,
            "html_url": format!("{WEB_URL}/issues/{number}"),
            "title": title,
            "body": body,
            "user": { "login": user.0, "type": user.1 },
            "state": "open",
            "state_reason": null,
            "labels": label_objects(labels),
            "created_at": now,
            "updated_at": now,
            "closed_at": null,
            "performed_via_github_app": performed_via(user),
        });
        self.issues.push(issue.clone());
        issue
    }

    fn add_comment(&mut self, number: u64, body: &str, user: (&str, &str), association: &str) {
        self.next_comment_id += 1;
        let id = self.next_comment_id;
        let created_at = self.tick();
        self.comments.push((
            number,
            json!({
                "id": id,
                "body": body,
                "user": { "login": user.0, "type": user.1 },
                "author_association": association,
                "created_at": created_at,
                "html_url": format!("{WEB_URL}/issues/{number}#issuecomment-{id}"),
                "performed_via_github_app": performed_via(user),
            }),
        ));
    }

    fn set_label(&mut self, number: u64, name: &str, present: bool) -> bool {
        let Some(issue) = self.issue_mut(number) else {
            return false;
        };
        let mut labels: Vec<Value> = issue["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|label| label["name"] != name)
            .cloned()
            .collect();
        let had = labels.len() != issue["labels"].as_array().map_or(0, Vec::len);
        if present {
            labels.push(json!({ "name": name }));
        }
        issue["labels"] = Value::Array(labels);
        had || present
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

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
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
        let wanted_state = query("state").unwrap_or_else(|| "open".to_string());
        let since = query("since");
        let per_page: usize = query("per_page").and_then(|v| v.parse().ok()).unwrap_or(30);
        let page: usize = query("page").and_then(|v| v.parse().ok()).unwrap_or(1);
        let matching: Vec<&Value> = state
            .issues
            .iter()
            .filter(|issue| wanted_state == "all" || issue["state"] == wanted_state.as_str())
            .filter(|issue| {
                since
                    .as_deref()
                    .is_none_or(|since| issue["updated_at"].as_str().is_some_and(|at| at >= since))
            })
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

    fn update_issue(state: &mut FakeState, number: u64, payload: &Value) -> ResponseTemplate {
        let now = state.tick();
        let Some(issue) = state.issue_mut(number) else {
            return ResponseTemplate::new(404);
        };
        for field in ["title", "body", "state", "state_reason"] {
            if let Some(value) = payload.get(field) {
                issue[field] = value.clone();
            }
        }
        if payload.get("state").and_then(Value::as_str) == Some("closed") {
            issue["closed_at"] = json!(now);
        }
        issue["updated_at"] = json!(now);
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
            .filter(|(issue, _)| *issue == number && page == 1)
            .map(|(_, comment)| comment)
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
            ("POST", ["issues"], _) => {
                let issue = state.add_issue(
                    payload["title"].as_str().unwrap_or_default(),
                    payload["body"].as_str().unwrap_or_default(),
                    &payload["labels"],
                    BOT,
                );
                ResponseTemplate::new(201).set_body_json(issue)
            }
            ("PATCH", ["issues", _], Some(number)) => {
                Self::update_issue(&mut state, number, &payload)
            }
            ("GET", ["issues", _, "comments"], Some(number)) => {
                Self::list_comments(&state, number, request)
            }
            ("POST", ["issues", _, "comments"], Some(number)) => {
                let body = payload["body"].as_str().unwrap_or_default().to_string();
                state.add_comment(number, &body, BOT, "NONE");
                ResponseTemplate::new(201).set_body_json(json!({}))
            }
            ("POST", ["issues", _, "labels"], Some(number)) => {
                for label in payload["labels"].as_array().into_iter().flatten() {
                    state.set_label(number, label.as_str().unwrap_or_default(), true);
                }
                ResponseTemplate::new(200).set_body_json(json!([]))
            }
            ("DELETE", ["issues", _, "labels", label], Some(number)) => {
                let label = label.replace("%3A", ":");
                if state.set_label(number, &label, false) {
                    ResponseTemplate::new(200).set_body_json(json!([]))
                } else {
                    ResponseTemplate::new(404)
                }
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
    fn repository(&self) -> Arc<dyn WorkStatePort> {
        self.repository.clone()
    }

    async fn create_as_human(&self, objective: &ObjectiveId, title: &str, body: &str) -> RecordRef {
        let issue = self.fake.lock().add_issue(
            title,
            body,
            &json!([format!("reili-objective:{objective}")]),
            ("alice", "User"),
        );
        let number = issue["number"].as_u64().unwrap_or_default();
        RecordRef {
            key: RecordKey::new(format!("#{number}")),
            url: format!("{WEB_URL}/issues/{number}"),
        }
    }

    async fn create_unlinked_as_human(&self, title: &str, body: &str) -> RecordRef {
        let issue = self
            .fake
            .lock()
            .add_issue(title, body, &json!([]), ("alice", "User"));
        let number = issue["number"].as_u64().unwrap_or_default();
        RecordRef {
            key: RecordKey::new(format!("#{number}")),
            url: format!("{WEB_URL}/issues/{number}"),
        }
    }

    async fn comment_as_human(&self, record: &RecordRef, author: &str, trusted: bool, body: &str) {
        let association = if trusted { "MEMBER" } else { "NONE" };
        self.fake
            .lock()
            .add_comment(issue_number(record), body, (author, "User"), association);
    }

    async fn set_assigned_to_reili_as_human(&self, record: &RecordRef, assigned: bool) {
        self.fake
            .lock()
            .set_label(issue_number(record), ASSIGNEE_LABEL, assigned);
    }
}
