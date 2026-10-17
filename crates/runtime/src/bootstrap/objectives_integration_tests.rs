//! Integration test: the production scheduler, cycle runner, and GitHub Issues adapter talking
//! HTTP to a stateful fake GitHub REST API. Covers durable work state, human collaboration on the
//! issue, process restarts, and that writes never leave the management repository.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{DateTime, Duration, Utc};
use reili_adapters::outbound::github::{GitHubWorkStateConfig, GitHubWorkStateRepository};
use reili_application::objective::{Clock, ObjectiveScheduler};
use reili_core::error::{AgentRunFailedError, PortError};
use reili_core::investigation::{
    InvestigationRunOutcome, InvestigationRunReport, InvestigationRunnerPort, RunInvestigationInput,
};
use reili_core::logger::LogEntry;
use reili_core::messaging::slack::{
    MockSlackFileDownloadPort, MockSlackMessageSearchPort, ObjectiveReportMessage,
    ObjectiveReportPort, PostedObjectiveReport,
};
use reili_core::objective::{
    ObjectiveBudget, ObjectiveDefinition, ObjectiveId, ObjectivePermissions, ObjectiveReportPolicy,
    ObjectiveSchedule, ObjectiveScope,
};
use reili_core::secret::SecretString;
use reili_core::task::{LlmExecutionMetadata, LlmUsageSnapshot, TaskResources};
use reili_core::work::{
    EvidenceRef, EvidenceSource, Hypothesis, HypothesisStatus, InvestigationReport, Severity,
    WorkItemKind, WorkItemProposal, WorkItemStatus,
};
use serde_json::{Value, json};

use super::{AssembleObjectiveSchedulerInput, assemble_objective_scheduler};
use crate::config::AutonomyConfig;

const OWNER: &str = "acme";
const REPO: &str = "reili-work";

#[derive(Default)]
struct FakeGitHubState {
    issues: BTreeMap<u64, Value>,
    comments: BTreeMap<u64, Vec<Value>>,
    next_number: u64,
    next_comment_id: u64,
    foreign_requests: Vec<String>,
    writes: Vec<String>,
}

type Shared = Arc<Mutex<FakeGitHubState>>;

fn check_repo(state: &Shared, method: &Method, owner: &str, repo: &str, uri: &Uri) -> bool {
    let mut guard = state.lock().expect("lock");
    if owner != OWNER || repo != REPO {
        guard.foreign_requests.push(format!("{method} {uri}"));
        return false;
    }
    if method != Method::GET {
        guard.writes.push(format!("{method} {}", uri.path()));
    }
    true
}

fn query_value(uri: &Uri, key: &str) -> Option<String> {
    uri.query()?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| {
            value
                .replace("%3A", ":")
                .replace("%2C", ",")
                .replace('+', " ")
        })
    })
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

async fn list_or_create_issues(
    State(state): State<Shared>,
    method: Method,
    uri: Uri,
    Path((owner, repo)): Path<(String, String)>,
    body: Option<Json<Value>>,
) -> Response {
    if !check_repo(&state, &method, &owner, &repo, &uri) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut guard = state.lock().expect("lock");
    if method == Method::GET {
        if query_value(&uri, "page").is_some_and(|page| page != "1") {
            return Json(json!([])).into_response();
        }
        let wanted: Vec<String> = query_value(&uri, "labels")
            .map(|labels| labels.split(',').map(ToString::to_string).collect())
            .unwrap_or_default();
        let issues: Vec<Value> = guard
            .issues
            .values()
            .filter(|issue| {
                let names: Vec<&str> = issue["labels"]
                    .as_array()
                    .map(|labels| {
                        labels
                            .iter()
                            .filter_map(|label| label["name"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                wanted.iter().all(|label| names.contains(&label.as_str()))
            })
            .cloned()
            .collect();
        return Json(Value::Array(issues)).into_response();
    }
    let Some(Json(body)) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    guard.next_number += 1;
    let number = guard.next_number;
    let issue = json!({
        "number": number,
        "html_url": format!("https://github.com/{OWNER}/{REPO}/issues/{number}"),
        "title": body["title"],
        "body": body["body"],
        "state": "open",
        "state_reason": null,
        "labels": body["labels"].as_array().cloned().unwrap_or_default().iter().map(|name| json!({ "name": name })).collect::<Vec<_>>(),
        "updated_at": now(),
    });
    guard.issues.insert(number, issue.clone());
    (StatusCode::CREATED, Json(issue)).into_response()
}

async fn get_or_update_issue(
    State(state): State<Shared>,
    method: Method,
    uri: Uri,
    Path((owner, repo, number)): Path<(String, String, u64)>,
    body: Option<Json<Value>>,
) -> Response {
    if !check_repo(&state, &method, &owner, &repo, &uri) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut guard = state.lock().expect("lock");
    let Some(issue) = guard.issues.get_mut(&number) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if method == Method::PATCH
        && let Some(Json(patch)) = body
    {
        for key in ["title", "body", "state", "state_reason"] {
            if let Some(value) = patch.get(key) {
                issue[key] = value.clone();
            }
        }
        if let Some(labels) = patch.get("labels").and_then(Value::as_array) {
            issue["labels"] =
                Value::Array(labels.iter().map(|name| json!({ "name": name })).collect());
        }
        issue["updated_at"] = json!(now());
    }
    Json(issue.clone()).into_response()
}

async fn comments(
    State(state): State<Shared>,
    method: Method,
    uri: Uri,
    Path((owner, repo, number)): Path<(String, String, u64)>,
    body: Option<Json<Value>>,
) -> Response {
    if !check_repo(&state, &method, &owner, &repo, &uri) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut guard = state.lock().expect("lock");
    if method == Method::GET {
        if query_value(&uri, "page").is_some_and(|page| page != "1") {
            return Json(json!([])).into_response();
        }
        return Json(Value::Array(
            guard.comments.get(&number).cloned().unwrap_or_default(),
        ))
        .into_response();
    }
    let Some(Json(body)) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    guard.next_comment_id += 1;
    let comment = json!({
        "id": guard.next_comment_id,
        "body": body["body"],
        "user": { "login": "reili[bot]", "type": "Bot" },
        "author_association": "NONE",
        "created_at": now(),
        "html_url": "https://github.com/acme/reili-work/issues/comment",
    });
    guard
        .comments
        .entry(number)
        .or_default()
        .push(comment.clone());
    (StatusCode::CREATED, Json(comment)).into_response()
}

async fn labels(
    State(state): State<Shared>,
    method: Method,
    uri: Uri,
    Path((owner, repo)): Path<(String, String)>,
) -> Response {
    if !check_repo(&state, &method, &owner, &repo, &uri) {
        return StatusCode::NOT_FOUND.into_response();
    }
    StatusCode::CREATED.into_response()
}

async fn fallback(State(state): State<Shared>, method: Method, uri: Uri) -> Response {
    state
        .lock()
        .expect("lock")
        .foreign_requests
        .push(format!("{method} {uri}"));
    StatusCode::NOT_FOUND.into_response()
}

async fn start_fake_github() -> (String, Shared) {
    let state: Shared = Arc::new(Mutex::new(FakeGitHubState::default()));
    let app = Router::new()
        .route(
            "/repos/{owner}/{repo}/issues",
            get(list_or_create_issues).post(list_or_create_issues),
        )
        .route(
            "/repos/{owner}/{repo}/issues/{number}",
            get(get_or_update_issue).patch(get_or_update_issue),
        )
        .route(
            "/repos/{owner}/{repo}/issues/{number}/comments",
            get(comments).post(comments),
        )
        .route("/repos/{owner}/{repo}/labels", post(labels))
        .fallback(fallback)
        .with_state(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}"), state)
}

struct ScriptedRunner {
    reports: Mutex<Vec<InvestigationReport>>,
    briefs: Mutex<Vec<String>>,
}

#[async_trait]
impl InvestigationRunnerPort for ScriptedRunner {
    async fn run(
        &self,
        input: RunInvestigationInput,
    ) -> Result<InvestigationRunOutcome, AgentRunFailedError> {
        self.briefs.lock().expect("lock").push(
            input
                .brief
                .sections
                .iter()
                .map(|section| section.body.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let report = {
            let mut reports = self.reports.lock().expect("lock");
            if reports.is_empty() {
                None
            } else {
                Some(reports.remove(0))
            }
        };
        Ok(InvestigationRunOutcome::Completed(Box::new(
            InvestigationRunReport {
                report,
                usage: LlmUsageSnapshot::default(),
                execution: LlmExecutionMetadata {
                    provider: "scripted".to_string(),
                    model: "scripted".to_string(),
                },
                final_text: String::new(),
            },
        )))
    }
}

#[derive(Default)]
struct RecordingSlack {
    posted: Mutex<Vec<ObjectiveReportMessage>>,
}

#[async_trait]
impl ObjectiveReportPort for RecordingSlack {
    async fn find_posted_report(
        &self,
        _channel_id: &str,
        report_key: &str,
    ) -> Result<Option<PostedObjectiveReport>, PortError> {
        Ok(self
            .posted
            .lock()
            .expect("lock")
            .iter()
            .find(|message| message.report_key == report_key)
            .map(|message| PostedObjectiveReport {
                channel_id: message.channel_id.clone(),
                ts: "1.0".to_string(),
            }))
    }

    async fn post_report(
        &self,
        message: ObjectiveReportMessage,
    ) -> Result<PostedObjectiveReport, PortError> {
        let channel_id = message.channel_id.clone();
        self.posted.lock().expect("lock").push(message);
        Ok(PostedObjectiveReport {
            channel_id,
            ts: "1.0".to_string(),
        })
    }

    async fn permalink(&self, _: &str, _: &str) -> Result<Option<String>, PortError> {
        Ok(None)
    }
}

struct QuietLogger;

impl reili_core::logger::Logger for QuietLogger {
    fn log(&self, _entry: LogEntry) {}
}

fn objective() -> ObjectiveDefinition {
    ObjectiveDefinition {
        id: ObjectiveId::new("checkout-reliability").expect("id"),
        description: "Continuously improve checkout service reliability.".to_string(),
        scope: ObjectiveScope {
            services: vec!["checkout-api".to_string()],
            repositories: Vec::new(),
            environments: Vec::new(),
        },
        schedule: ObjectiveSchedule::parse("0 */6 * * *", "Asia/Tokyo").expect("schedule"),
        report: ObjectiveReportPolicy {
            channel_id: "C0123456789".to_string(),
            min_interval_minutes: 60,
            max_reports_per_day: 6,
        },
        budget: ObjectiveBudget::default(),
        permissions: ObjectivePermissions {
            save_knowledge: false,
            post_reports: true,
        },
        enabled: true,
    }
}

fn report() -> InvestigationReport {
    InvestigationReport {
        summary: "checkout-api errors rose after deploy 42.".to_string(),
        evidence: vec![EvidenceRef {
            id: "E1".to_string(),
            source: EvidenceSource::Datadog,
            url: Some("https://app.datadoghq.com/apm/services/checkout-api".to_string()),
            description: "error rate".to_string(),
            observed_at: None,
            time_range: None,
            query: None,
        }],
        work_items: vec![WorkItemProposal {
            key: "checkout-api:errors-after-deploy-42".to_string(),
            existing_issue_number: None,
            title: "Errors after deploy 42".to_string(),
            kind: WorkItemKind::Incident,
            severity: Severity::High,
            proposed_status: WorkItemStatus::NeedsHuman,
            problem_statement: "5xx doubled".to_string(),
            plan: Vec::new(),
            hypotheses: vec![Hypothesis {
                id: "H1".to_string(),
                statement: "deploy 42 caused the regression".to_string(),
                status: HypothesisStatus::Supported,
                evidence_ids: vec!["E1".to_string()],
                rationale: String::new(),
            }],
            findings: Vec::new(),
            recommendation: None,
            open_questions: Vec::new(),
            human_decision_needed: Some("Roll back deploy 42?".to_string()),
            next_actions: Vec::new(),
            evidence_ids: vec!["E1".to_string()],
        }],
        knowledge_candidates: Vec::new(),
        no_new_findings: false,
    }
}

/// Process clock that can be moved forward to reach the next scheduled slot.
struct SteppedClock(Mutex<DateTime<Utc>>);

impl SteppedClock {
    fn advance(&self, by: Duration) {
        *self.0.lock().expect("lock") += by;
    }
}

impl Clock for SteppedClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().expect("lock")
    }
}

fn process(
    api_base_url: &str,
    runner: &Arc<ScriptedRunner>,
    slack: &Arc<RecordingSlack>,
    clock: &Arc<SteppedClock>,
    instance_id: &str,
) -> Arc<ObjectiveScheduler> {
    let repository = GitHubWorkStateRepository::with_static_token(
        &GitHubWorkStateConfig {
            repository: format!("{OWNER}/{REPO}"),
            app_id: "1".to_string(),
            private_key: SecretString::from("unused"),
            installation_id: 1,
            api_base_url: api_base_url.to_string(),
            web_base_url: "https://github.com".to_string(),
        },
        SecretString::from("token"),
    )
    .expect("repository");
    assemble_objective_scheduler(AssembleObjectiveSchedulerInput {
        objectives: vec![objective()],
        autonomy: &AutonomyConfig {
            tick_interval_seconds: 60,
            max_concurrent_cycles: 1,
            lease_minutes: 15,
        },
        repository: Arc::new(repository),
        investigation_runner: Arc::clone(runner) as Arc<dyn InvestigationRunnerPort>,
        task_resources: TaskResources {
            slack_message_search_port: Arc::new(MockSlackMessageSearchPort::new()),
            slack_file_download_port: Arc::new(MockSlackFileDownloadPort::new()),
            web_search_port: None,
            canvas_memory_port: None,
        },
        report_port: Arc::clone(slack) as Arc<dyn ObjectiveReportPort>,
        clock: Arc::clone(clock) as Arc<dyn Clock>,
        logger: Arc::new(QuietLogger),
        instance_id: instance_id.to_string(),
    })
}

async fn run_tick(scheduler: &Arc<ObjectiveScheduler>) -> usize {
    let handles = scheduler.tick().await;
    let count = handles.len();
    for handle in handles {
        handle.await.expect("cycle");
    }
    count
}

fn labels_of(issue: &Value) -> Vec<String> {
    issue["labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label["name"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn objective_cycles_persist_to_github_issues_and_resume_across_processes() {
    let (api_base_url, github) = start_fake_github().await;
    let runner = Arc::new(ScriptedRunner {
        reports: Mutex::new(vec![report()]),
        briefs: Mutex::new(Vec::new()),
    });
    let slack = Arc::new(RecordingSlack::default());

    // Process 1: baseline cycle creates the ledger and a work item over HTTP.
    let clock = Arc::new(SteppedClock(Mutex::new(Utc::now())));
    let first = process(&api_base_url, &runner, &slack, &clock, "process-1");
    assert_eq!(run_tick(&first).await, 1);

    let (ledger_number, item_number) = {
        let state = github.lock().expect("lock");
        let ledger = state
            .issues
            .values()
            .find(|issue| labels_of(issue).contains(&"reili:objective".to_string()))
            .expect("ledger issue");
        let item = state
            .issues
            .values()
            .find(|issue| labels_of(issue).contains(&"reili:work-item".to_string()))
            .expect("work item issue");
        assert!(labels_of(item).contains(&"reili-status:needs-human".to_string()));
        assert!(labels_of(item).contains(&"reili-objective:checkout-reliability".to_string()));
        let body = item["body"].as_str().expect("body");
        assert!(body.contains("### Decision needed\nRoll back deploy 42?"));
        assert!(body.contains("| supported |"));
        (
            ledger["number"].as_u64().expect("number"),
            item["number"].as_u64().expect("number"),
        )
    };
    assert_eq!(slack.posted.lock().expect("lock").len(), 1);

    // A human comments, moves the item to `proposed` by label, and adds a label of their own.
    {
        let mut state = github.lock().expect("lock");
        state.next_comment_id += 1;
        let id = state.next_comment_id;
        state.comments.entry(item_number).or_default().push(json!({
            "id": id,
            "body": "Confirmed, the pool size is too small.",
            "user": { "login": "alice", "type": "User" },
            "author_association": "MEMBER",
            "created_at": now(),
            "html_url": "https://github.com/acme/reili-work/issues/comment",
        }));
        let issue = state.issues.get_mut(&item_number).expect("issue");
        let mut labels: Vec<Value> = issue["labels"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|label| {
                !label["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("reili-status:"))
            })
            .collect();
        labels.push(json!({ "name": "reili-status:proposed" }));
        labels.push(json!({ "name": "customer-facing" }));
        issue["labels"] = Value::Array(labels);
    }

    // Process 2 (restart) at the next scheduled slot: durable state comes from GitHub only.
    let second = process(&api_base_url, &runner, &slack, &clock, "process-2");
    clock.advance(Duration::hours(7));
    assert_eq!(run_tick(&second).await, 1);

    let brief = runner
        .briefs
        .lock()
        .expect("lock")
        .last()
        .cloned()
        .unwrap_or_default();
    assert!(
        brief.contains("Confirmed, the pool size is too small."),
        "feedback reaches the agent"
    );
    let state = github.lock().expect("lock");
    let item = &state.issues[&item_number];
    let labels = labels_of(item);
    assert!(labels.contains(&"reili-status:proposed".to_string()));
    assert!(
        labels.contains(&"customer-facing".to_string()),
        "human labels preserved"
    );
    assert!(state.comments[&item_number].iter().any(|comment| {
        comment["body"]
            .as_str()
            .is_some_and(|body| body.contains("Recorded human status change → `proposed`"))
    }));
    let ledger_body = state.issues[&ledger_number]["body"]
        .as_str()
        .expect("ledger body");
    assert_eq!(
        ledger_body.matches("\"run_id\"").count(),
        2,
        "both cycles are recorded in the ledger"
    );
    assert_eq!(
        state
            .issues
            .values()
            .filter(|issue| labels_of(issue).contains(&"reili:work-item".to_string()))
            .count(),
        1,
        "no duplicate work items across processes"
    );
    assert!(
        state.foreign_requests.is_empty(),
        "{:?}",
        state.foreign_requests
    );
    assert!(
        state
            .writes
            .iter()
            .all(|write| write.contains(&format!("/repos/{OWNER}/{REPO}/"))),
        "all writes target the management repository"
    );
}
