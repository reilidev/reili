//! In-memory test doubles that mimic the semantics the production adapters rely on: GitHub-style
//! revision checks, label-derived status, human comments, and durability across "restarts"
//! (a new runner instance sharing the same fake).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use reili_core::error::{AgentRunFailedError, PortError};
use reili_core::goal::GoalId;
use reili_core::investigation::{
    InvestigationRunOutcome, InvestigationRunReport, InvestigationRunnerPort, RunInvestigationInput,
};
use reili_core::logger::LogEntry;
use reili_core::messaging::slack::{
    AppendSlackCanvasMemoryInput, GoalReportMessage, GoalReportPort, ListSlackCanvasMemoriesInput,
    PostedGoalReport, SlackCanvasMemoryPort, SlackCanvasMemoryRecord, SlackCanvasMemoryVisibility,
};
use reili_core::task::{LlmExecutionMetadata, LlmUsageSnapshot};
use reili_core::work::{
    GoalLedgerState, HumanComment, InvestigationReport, IssueRef, StoredLedger, StoredWorkItem,
    WorkItemState, WorkItemStatus, WorkStateComment, WorkStateError, WorkStateRepositoryPort,
};

use super::clock::Clock;
use crate::TaskLogger;

pub(crate) fn at(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("parse time")
        .with_timezone(&Utc)
}

#[derive(Clone)]
pub(crate) struct ManualClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl ManualClock {
    pub(crate) fn new(now: DateTime<Utc>) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
        }
    }

    pub(crate) fn advance(&self, duration: Duration) {
        let mut now = self.now.lock().expect("lock clock");
        *now += duration;
    }

    pub(crate) fn now_for_test(&self) -> DateTime<Utc> {
        Clock::now(self)
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("lock clock")
    }
}

pub(crate) struct NoopLogger;

impl TaskLogger for NoopLogger {
    fn log(&self, _entry: LogEntry) {}
}

#[derive(Debug, Clone)]
pub(crate) struct FakeComment {
    pub id: u64,
    pub author: String,
    pub trusted: bool,
    pub body: String,
    pub by_reili: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum FakeIssueBody {
    Ledger(String),
    WorkItem(String),
}

#[derive(Debug, Clone)]
pub(crate) struct FakeIssue {
    pub number: u64,
    pub goal_id: GoalId,
    pub body: FakeIssueBody,
    pub human_notes: String,
    pub observed_status: Option<WorkItemStatus>,
    pub closed: bool,
    pub duplicate_of: Option<u64>,
    pub comments: Vec<FakeComment>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Default)]
struct FakeRepositoryState {
    issues: BTreeMap<u64, FakeIssue>,
    next_number: u64,
    next_comment_id: u64,
    unavailable: bool,
    writes: u64,
    /// Number of upcoming `save_*` calls that report a conflict (simulates a concurrent writer).
    injected_conflicts: u32,
    /// Creates a racing duplicate on the next work item create.
    race_next_create: bool,
}

/// GitHub-Issues-like in-memory repository. State is stored as serialized JSON, so every read
/// goes through a serialization round trip like the real adapter.
#[derive(Clone, Default)]
pub(crate) struct FakeWorkStateRepository {
    state: Arc<Mutex<FakeRepositoryState>>,
    clock: Option<ManualClock>,
}

impl FakeWorkStateRepository {
    pub(crate) fn new(clock: ManualClock) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeRepositoryState {
                next_number: 1,
                next_comment_id: 1,
                ..FakeRepositoryState::default()
            })),
            clock: Some(clock),
        }
    }

    fn now(&self) -> DateTime<Utc> {
        self.clock.as_ref().map_or_else(
            || Utc.timestamp_opt(0, 0).single().expect("time"),
            Clock::now,
        )
    }

    pub(crate) fn set_unavailable(&self, unavailable: bool) {
        self.state.lock().expect("lock").unavailable = unavailable;
    }

    pub(crate) fn inject_conflicts(&self, count: u32) {
        self.state.lock().expect("lock").injected_conflicts = count;
    }

    pub(crate) fn race_next_create(&self) {
        self.state.lock().expect("lock").race_next_create = true;
    }

    pub(crate) fn issues(&self) -> Vec<FakeIssue> {
        self.state
            .lock()
            .expect("lock")
            .issues
            .values()
            .cloned()
            .collect()
    }

    pub(crate) fn work_items(&self) -> Vec<WorkItemState> {
        self.issues()
            .into_iter()
            .filter_map(|issue| match issue.body {
                FakeIssueBody::WorkItem(json) => serde_json::from_str(&json).ok(),
                FakeIssueBody::Ledger(_) => None,
            })
            .collect()
    }

    pub(crate) fn open_work_items(&self) -> Vec<(u64, WorkItemState)> {
        self.issues()
            .into_iter()
            .filter(|issue| !issue.closed)
            .filter_map(|issue| match issue.body {
                FakeIssueBody::WorkItem(json) => serde_json::from_str(&json)
                    .ok()
                    .map(|state| (issue.number, state)),
                FakeIssueBody::Ledger(_) => None,
            })
            .collect()
    }

    pub(crate) fn ledger(&self, goal_id: &str) -> Option<GoalLedgerState> {
        self.issues()
            .into_iter()
            .find_map(|issue| match issue.body {
                FakeIssueBody::Ledger(json)
                    if issue.goal_id.as_str() == goal_id && !issue.closed =>
                {
                    serde_json::from_str(&json).ok()
                }
                _ => None,
            })
    }

    pub(crate) fn comments(&self, number: u64) -> Vec<FakeComment> {
        self.state
            .lock()
            .expect("lock")
            .issues
            .get(&number)
            .map(|issue| issue.comments.clone())
            .unwrap_or_default()
    }

    /// Simulates a human changing the status label (or closing the issue).
    pub(crate) fn human_set_status(&self, number: u64, status: WorkItemStatus) {
        let now = self.now();
        let mut state = self.state.lock().expect("lock");
        if let Some(issue) = state.issues.get_mut(&number) {
            issue.observed_status = Some(status);
            issue.closed = !status.is_open();
            issue.updated_at = now;
        }
    }

    pub(crate) fn human_comment(&self, number: u64, author: &str, trusted: bool, body: &str) {
        let now = self.now();
        let mut state = self.state.lock().expect("lock");
        let id = state.next_comment_id;
        state.next_comment_id += 1;
        if let Some(issue) = state.issues.get_mut(&number) {
            issue.comments.push(FakeComment {
                id,
                author: author.to_string(),
                trusted,
                body: body.to_string(),
                by_reili: false,
            });
            issue.updated_at = now;
        }
    }

    fn check_available(state: &FakeRepositoryState) -> Result<(), WorkStateError> {
        if state.unavailable {
            return Err(WorkStateError::Unavailable(PortError::connection_failed(
                "GitHub API unreachable",
            )));
        }
        Ok(())
    }

    fn issue_ref(number: u64) -> IssueRef {
        IssueRef {
            number,
            url: format!("https://github.com/acme/reili-work/issues/{number}"),
        }
    }

    fn to_ledger(issue: &FakeIssue) -> Result<StoredLedger, WorkStateError> {
        let FakeIssueBody::Ledger(json) = &issue.body else {
            return Err(WorkStateError::Corrupted {
                issue_number: issue.number,
                message: "not a ledger".to_string(),
            });
        };
        Ok(StoredLedger {
            issue: Self::issue_ref(issue.number),
            state: serde_json::from_str(json).map_err(|error| WorkStateError::Corrupted {
                issue_number: issue.number,
                message: error.to_string(),
            })?,
            human_notes: issue.human_notes.clone(),
        })
    }

    fn to_work_item(issue: &FakeIssue) -> Result<StoredWorkItem, WorkStateError> {
        let FakeIssueBody::WorkItem(json) = &issue.body else {
            return Err(WorkStateError::Corrupted {
                issue_number: issue.number,
                message: "not a work item".to_string(),
            });
        };
        let state: WorkItemState =
            serde_json::from_str(json).map_err(|error| WorkStateError::Corrupted {
                issue_number: issue.number,
                message: error.to_string(),
            })?;
        let observed_status = match issue.observed_status {
            Some(status) => status,
            None => state.status,
        };
        let observed_status = if issue.closed && observed_status.is_open() {
            WorkItemStatus::Resolved
        } else if !issue.closed && !observed_status.is_open() {
            WorkItemStatus::Investigating
        } else {
            observed_status
        };
        Ok(StoredWorkItem {
            issue: Self::issue_ref(issue.number),
            state,
            human_notes: issue.human_notes.clone(),
            observed_status,
            remote_updated_at: issue.updated_at,
        })
    }
}

#[async_trait]
impl WorkStateRepositoryPort for FakeWorkStateRepository {
    async fn find_goal_ledger(
        &self,
        goal_id: &GoalId,
    ) -> Result<Option<StoredLedger>, WorkStateError> {
        let state = self.state.lock().expect("lock");
        Self::check_available(&state)?;
        state
            .issues
            .values()
            .filter(|issue| {
                &issue.goal_id == goal_id
                    && matches!(issue.body, FakeIssueBody::Ledger(_))
                    && issue.duplicate_of.is_none()
            })
            .min_by_key(|issue| issue.number)
            .map(Self::to_ledger)
            .transpose()
    }

    async fn list_goal_ledgers(&self) -> Result<Vec<StoredLedger>, WorkStateError> {
        let state = self.state.lock().expect("lock");
        Self::check_available(&state)?;
        state
            .issues
            .values()
            .filter(|issue| {
                matches!(issue.body, FakeIssueBody::Ledger(_)) && issue.duplicate_of.is_none()
            })
            .map(Self::to_ledger)
            .collect()
    }

    async fn create_goal_ledger(
        &self,
        ledger: &GoalLedgerState,
    ) -> Result<StoredLedger, WorkStateError> {
        let now = self.now();
        let mut state = self.state.lock().expect("lock");
        Self::check_available(&state)?;
        let number = state.next_number;
        state.next_number += 1;
        state.writes += 1;
        let mut stored = ledger.clone();
        stored.revision = 1;
        let issue = FakeIssue {
            number,
            goal_id: ledger.goal_id.clone(),
            body: FakeIssueBody::Ledger(serde_json::to_string(&stored).expect("json")),
            human_notes: String::new(),
            observed_status: None,
            closed: false,
            duplicate_of: None,
            comments: Vec::new(),
            updated_at: now,
        };
        let result = Self::to_ledger(&issue);
        state.issues.insert(number, issue);
        result
    }

    async fn save_goal_ledger(
        &self,
        issue: &IssueRef,
        expected_revision: u64,
        ledger: &GoalLedgerState,
    ) -> Result<StoredLedger, WorkStateError> {
        let now = self.now();
        let mut state = self.state.lock().expect("lock");
        Self::check_available(&state)?;
        if state.injected_conflicts > 0 {
            state.injected_conflicts -= 1;
            let stored = state.issues.get_mut(&issue.number).expect("issue");
            let FakeIssueBody::Ledger(json) = &stored.body else {
                panic!("not a ledger");
            };
            let mut current: GoalLedgerState = serde_json::from_str(json).expect("json");
            current.revision += 1;
            stored.body = FakeIssueBody::Ledger(serde_json::to_string(&current).expect("json"));
            return Err(WorkStateError::Conflict {
                issue_number: issue.number,
                expected: expected_revision,
                found: current.revision,
            });
        }
        let stored = state.issues.get_mut(&issue.number).expect("issue");
        let current = Self::to_ledger(stored)?;
        if current.state.revision != expected_revision {
            return Err(WorkStateError::Conflict {
                issue_number: issue.number,
                expected: expected_revision,
                found: current.state.revision,
            });
        }
        let mut next = ledger.clone();
        next.revision = expected_revision + 1;
        stored.body = FakeIssueBody::Ledger(serde_json::to_string(&next).expect("json"));
        stored.updated_at = now;
        let result = Self::to_ledger(stored);
        state.writes += 1;
        result
    }

    async fn list_work_items(
        &self,
        goal_id: &GoalId,
    ) -> Result<Vec<StoredWorkItem>, WorkStateError> {
        let state = self.state.lock().expect("lock");
        Self::check_available(&state)?;
        state
            .issues
            .values()
            .filter(|issue| {
                &issue.goal_id == goal_id
                    && matches!(issue.body, FakeIssueBody::WorkItem(_))
                    && issue.duplicate_of.is_none()
            })
            .map(Self::to_work_item)
            .collect()
    }

    async fn create_work_item(
        &self,
        item: &WorkItemState,
    ) -> Result<StoredWorkItem, WorkStateError> {
        let now = self.now();
        let mut state = self.state.lock().expect("lock");
        Self::check_available(&state)?;
        let mut stored = item.clone();
        stored.revision = 1;
        let make_issue = |number: u64| FakeIssue {
            number,
            goal_id: item.goal_id.clone(),
            body: FakeIssueBody::WorkItem(serde_json::to_string(&stored).expect("json")),
            human_notes: String::new(),
            observed_status: None,
            closed: !item.status.is_open(),
            duplicate_of: None,
            comments: Vec::new(),
            updated_at: now,
        };
        if state.race_next_create {
            state.race_next_create = false;
            let racing = state.next_number;
            state.next_number += 1;
            state.issues.insert(racing, make_issue(racing));
        }
        let number = state.next_number;
        state.next_number += 1;
        state.writes += 1;
        let issue = make_issue(number);
        let result = Self::to_work_item(&issue);
        state.issues.insert(number, issue);
        result
    }

    async fn save_work_item(
        &self,
        issue: &IssueRef,
        expected_revision: u64,
        item: &WorkItemState,
    ) -> Result<StoredWorkItem, WorkStateError> {
        let now = self.now();
        let mut state = self.state.lock().expect("lock");
        Self::check_available(&state)?;
        let stored = state.issues.get_mut(&issue.number).expect("issue");
        let current = Self::to_work_item(stored)?;
        if current.state.revision != expected_revision {
            return Err(WorkStateError::Conflict {
                issue_number: issue.number,
                expected: expected_revision,
                found: current.state.revision,
            });
        }
        let mut next = item.clone();
        next.revision = expected_revision + 1;
        stored.body = FakeIssueBody::WorkItem(serde_json::to_string(&next).expect("json"));
        stored.observed_status = Some(next.status);
        stored.closed = !next.status.is_open();
        stored.updated_at = now;
        let result = Self::to_work_item(stored);
        state.writes += 1;
        result
    }

    async fn append_comment(
        &self,
        issue: &IssueRef,
        comment: WorkStateComment,
    ) -> Result<(), WorkStateError> {
        let now = self.now();
        let mut state = self.state.lock().expect("lock");
        Self::check_available(&state)?;
        let id = state.next_comment_id;
        state.next_comment_id += 1;
        state.writes += 1;
        let stored = state.issues.get_mut(&issue.number).expect("issue");
        stored.comments.push(FakeComment {
            id,
            author: "reili[bot]".to_string(),
            trusted: true,
            body: format!("[{}] {}", comment.kind.as_str(), comment.body),
            by_reili: true,
        });
        stored.updated_at = now;
        Ok(())
    }

    async fn list_human_comments(
        &self,
        issue: &IssueRef,
        after_comment_id: Option<u64>,
    ) -> Result<Vec<HumanComment>, WorkStateError> {
        let state = self.state.lock().expect("lock");
        Self::check_available(&state)?;
        let stored = state.issues.get(&issue.number).expect("issue");
        Ok(stored
            .comments
            .iter()
            .filter(|comment| !comment.by_reili)
            .filter(|comment| after_comment_id.is_none_or(|after| comment.id > after))
            .map(|comment| HumanComment {
                id: comment.id,
                author: comment.author.clone(),
                author_trusted: comment.trusted,
                body: comment.body.clone(),
                created_at: stored.updated_at,
                url: format!(
                    "{}#issuecomment-{}",
                    Self::issue_ref(issue.number).url,
                    comment.id
                ),
            })
            .collect())
    }

    async fn close_duplicate(
        &self,
        duplicate: &IssueRef,
        canonical: &IssueRef,
    ) -> Result<(), WorkStateError> {
        let mut state = self.state.lock().expect("lock");
        Self::check_available(&state)?;
        let stored = state.issues.get_mut(&duplicate.number).expect("issue");
        stored.closed = true;
        stored.duplicate_of = Some(canonical.number);
        Ok(())
    }
}

/// Scripted investigation runner: returns queued reports in order and records each brief.
#[derive(Clone, Default)]
pub(crate) struct ScriptedInvestigationRunner {
    responses: Arc<Mutex<Vec<ScriptedResponse>>>,
    pub briefs: Arc<Mutex<Vec<String>>>,
}

pub(crate) enum ScriptedResponse {
    Report {
        report: Option<InvestigationReport>,
        tokens: u64,
    },
    Fail(String),
    Hang,
}

impl ScriptedInvestigationRunner {
    pub(crate) fn push(&self, response: ScriptedResponse) {
        self.responses.lock().expect("lock").push(response);
    }

    pub(crate) fn push_report(&self, report: InvestigationReport, tokens: u64) {
        self.push(ScriptedResponse::Report {
            report: Some(report),
            tokens,
        });
    }

    pub(crate) fn calls(&self) -> usize {
        self.briefs.lock().expect("lock").len()
    }

    pub(crate) fn last_brief(&self) -> String {
        self.briefs
            .lock()
            .expect("lock")
            .last()
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait]
impl InvestigationRunnerPort for ScriptedInvestigationRunner {
    async fn run(
        &self,
        input: RunInvestigationInput,
    ) -> Result<InvestigationRunOutcome, AgentRunFailedError> {
        let rendered = input
            .brief
            .sections
            .iter()
            .map(|section| format!("## {}\n{}", section.heading, section.body))
            .collect::<Vec<_>>()
            .join("\n\n");
        self.briefs.lock().expect("lock").push(rendered);
        let response = {
            let mut responses = self.responses.lock().expect("lock");
            if responses.is_empty() {
                None
            } else {
                Some(responses.remove(0))
            }
        };
        match response {
            None => Ok(InvestigationRunOutcome::Completed(Box::new(
                InvestigationRunReport {
                    report: Some(InvestigationReport {
                        summary: "No new findings.".to_string(),
                        evidence: Vec::new(),
                        work_items: Vec::new(),
                        knowledge_candidates: Vec::new(),
                        no_new_findings: true,
                    }),
                    usage: LlmUsageSnapshot::default(),
                    execution: execution(),
                    final_text: String::new(),
                },
            ))),
            Some(ScriptedResponse::Report { report, tokens }) => {
                input.budget.record_tokens(tokens);
                Ok(InvestigationRunOutcome::Completed(Box::new(
                    InvestigationRunReport {
                        report,
                        usage: LlmUsageSnapshot {
                            requests: 1,
                            input_tokens: tokens,
                            output_tokens: 0,
                            total_tokens: tokens,
                        },
                        execution: execution(),
                        final_text: String::new(),
                    },
                )))
            }
            Some(ScriptedResponse::Fail(message)) => Err(AgentRunFailedError::new(
                LlmUsageSnapshot::default(),
                message,
            )),
            Some(ScriptedResponse::Hang) => {
                input.cancellation.wait_for_cancellation().await;
                Ok(InvestigationRunOutcome::Cancelled)
            }
        }
    }
}

fn execution() -> LlmExecutionMetadata {
    LlmExecutionMetadata {
        provider: "scripted".to_string(),
        model: "scripted".to_string(),
    }
}

#[derive(Clone, Default)]
pub(crate) struct RecordingReportPort {
    pub posted: Arc<Mutex<Vec<GoalReportMessage>>>,
    pub fail_posts: Arc<Mutex<bool>>,
}

impl RecordingReportPort {
    pub(crate) fn posted(&self) -> Vec<GoalReportMessage> {
        self.posted.lock().expect("lock").clone()
    }
}

#[async_trait]
impl GoalReportPort for RecordingReportPort {
    async fn find_posted_report(
        &self,
        channel_id: &str,
        report_key: &str,
    ) -> Result<Option<PostedGoalReport>, PortError> {
        Ok(self
            .posted
            .lock()
            .expect("lock")
            .iter()
            .enumerate()
            .find(|(_, message)| {
                message.channel_id == channel_id && message.report_key == report_key
            })
            .map(|(index, message)| PostedGoalReport {
                channel_id: message.channel_id.clone(),
                ts: format!("1760000000.{index:06}"),
            }))
    }

    async fn post_report(&self, message: GoalReportMessage) -> Result<PostedGoalReport, PortError> {
        if *self.fail_posts.lock().expect("lock") {
            return Err(PortError::connection_failed("slack down"));
        }
        let mut posted = self.posted.lock().expect("lock");
        let ts = format!("1760000000.{:06}", posted.len());
        let channel_id = message.channel_id.clone();
        posted.push(message);
        Ok(PostedGoalReport { channel_id, ts })
    }

    async fn permalink(&self, channel_id: &str, ts: &str) -> Result<Option<String>, PortError> {
        Ok(Some(format!(
            "https://acme.slack.com/archives/{channel_id}/p{}",
            ts.replace('.', "")
        )))
    }
}

#[derive(Clone, Default)]
pub(crate) struct RecordingCanvasMemory {
    pub appended: Arc<Mutex<Vec<AppendSlackCanvasMemoryInput>>>,
    pub memories: Arc<Mutex<Vec<SlackCanvasMemoryRecord>>>,
}

impl RecordingCanvasMemory {
    pub(crate) fn with_memory(fact: &str) -> Self {
        let memory = Self::default();
        memory
            .memories
            .lock()
            .expect("lock")
            .push(SlackCanvasMemoryRecord {
                visibility: SlackCanvasMemoryVisibility::Shared,
                fact: fact.to_string(),
                evidence: "runbook".to_string(),
                scope: "checkout".to_string(),
                source_url: Some("https://wiki/runbook".to_string()),
                created_at: "2026-10-01T00:00:00Z".to_string(),
            });
        memory
    }
}

#[async_trait]
impl SlackCanvasMemoryPort for RecordingCanvasMemory {
    async fn list_channel_memories(
        &self,
        _input: ListSlackCanvasMemoriesInput,
    ) -> Result<Vec<SlackCanvasMemoryRecord>, PortError> {
        Ok(self.memories.lock().expect("lock").clone())
    }

    async fn append_memory(&self, input: AppendSlackCanvasMemoryInput) -> Result<(), PortError> {
        self.appended.lock().expect("lock").push(input);
        Ok(())
    }
}
