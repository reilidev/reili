use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use reili_core::cycle_agent::{
    CycleAgentOutcome, CycleAgentPort, CycleAgentRun, RunCycleAgentInput,
};
use reili_core::error::{AgentRunFailedError, PortError};
use reili_core::logger::LogEntry;
use reili_core::messaging::slack::{
    AppendSlackCanvasMemoryInput, ListSlackCanvasMemoriesInput, SlackCanvasMemoryPort,
    SlackCanvasMemoryRecord, SlackCanvasMemoryVisibility, SlackPostMessageInput,
    SlackPostMessagePort,
};
use reili_core::objective::ObjectiveId;
use reili_core::task::LlmUsageSnapshot;
use reili_core::work::{
    CycleReport, NewWorkItem, RecordKey, RecordRef, WorkItem, WorkItemComment, WorkItemState,
    WorkStateError, WorkStatePort,
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

const REILI_LOGIN: &str = "reili[bot]";

#[derive(Debug, Clone)]
pub(crate) struct FakeComment {
    pub author: String,
    pub by_reili: bool,
    pub trusted: bool,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct FakeItem {
    pub number: u64,
    pub objective_id: Option<ObjectiveId>,
    pub title: String,
    pub body: String,
    pub author: String,
    pub state: WorkItemState,
    pub assigned_to_reili: bool,
    pub needs_decision: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub comments: Vec<FakeComment>,
}

#[derive(Default)]
struct FakeTrackerState {
    items: BTreeMap<u64, FakeItem>,
    next_number: u64,
    unavailable: bool,
}

#[derive(Clone)]
pub(crate) struct FakeTracker {
    state: Arc<Mutex<FakeTrackerState>>,
    clock: ManualClock,
}

impl FakeTracker {
    pub(crate) fn new(clock: ManualClock) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeTrackerState::default())),
            clock,
        }
    }

    pub(crate) fn number_of(record: &RecordRef) -> u64 {
        record
            .key
            .as_str()
            .trim_start_matches('#')
            .parse()
            .expect("fake records use `#<number>` keys")
    }

    fn record(number: u64) -> RecordRef {
        RecordRef {
            key: RecordKey::new(format!("#{number}")),
            url: format!("https://github.com/acme/reili-work/issues/{number}"),
        }
    }

    pub(crate) fn set_unavailable(&self, unavailable: bool) {
        self.state.lock().expect("lock").unavailable = unavailable;
    }

    pub(crate) fn items(&self) -> Vec<FakeItem> {
        self.state
            .lock()
            .expect("lock")
            .items
            .values()
            .cloned()
            .collect()
    }

    pub(crate) fn item(&self, number: u64) -> FakeItem {
        self.state
            .lock()
            .expect("lock")
            .items
            .get(&number)
            .cloned()
            .expect("item exists")
    }

    fn insert(&self, state: &mut FakeTrackerState, mut item: FakeItem) -> u64 {
        state.next_number += 1;
        item.number = state.next_number;
        let number = item.number;
        state.items.insert(number, item);
        number
    }

    fn new_item(
        &self,
        objective: Option<&ObjectiveId>,
        title: &str,
        body: &str,
        author: &str,
    ) -> FakeItem {
        let now = self.clock.now();
        FakeItem {
            number: 0,
            objective_id: objective.cloned(),
            title: title.to_string(),
            body: body.to_string(),
            author: author.to_string(),
            state: WorkItemState::Open,
            assigned_to_reili: false,
            needs_decision: false,
            created_at: now,
            updated_at: now,
            closed_at: None,
            comments: Vec::new(),
        }
    }

    pub(crate) fn human_create(
        &self,
        objective: &str,
        title: &str,
        body: &str,
        assigned: bool,
    ) -> u64 {
        let objective = ObjectiveId::new(objective).expect("objective id");
        let mut item = self.new_item(Some(&objective), title, body, "alice");
        item.assigned_to_reili = assigned;
        let mut state = self.state.lock().expect("lock");
        self.insert(&mut state, item)
    }

    /// A team member opens an item without any objective label.
    pub(crate) fn human_create_unlinked(&self, title: &str, body: &str, assigned: bool) -> u64 {
        let mut item = self.new_item(None, title, body, "alice");
        item.assigned_to_reili = assigned;
        let mut state = self.state.lock().expect("lock");
        self.insert(&mut state, item)
    }

    pub(crate) fn human_comment(&self, number: u64, author: &str, trusted: bool, body: &str) {
        let now = self.clock.now();
        self.update(number, |item| {
            item.comments.push(FakeComment {
                author: author.to_string(),
                by_reili: false,
                trusted,
                body: body.to_string(),
                created_at: now,
            });
        });
    }

    pub(crate) fn human_assign(&self, number: u64, assigned: bool) {
        self.update(number, |item| item.assigned_to_reili = assigned);
    }

    pub(crate) fn human_close(&self, number: u64, state: WorkItemState) {
        let now = self.clock.now();
        self.update(number, |item| {
            item.state = state;
            item.closed_at = Some(now);
        });
    }

    fn update(&self, number: u64, change: impl FnOnce(&mut FakeItem)) {
        let now = self.clock.now();
        let mut state = self.state.lock().expect("lock");
        let item = state.items.get_mut(&number).expect("item exists");
        change(item);
        item.updated_at = now;
    }

    fn check_available(&self) -> Result<(), WorkStateError> {
        if self.state.lock().expect("lock").unavailable {
            return Err(WorkStateError::Unavailable(PortError::connection_failed(
                "tracker down",
            )));
        }
        Ok(())
    }

    fn to_work_item(item: &FakeItem) -> WorkItem {
        WorkItem {
            record: Self::record(item.number),
            objective_id: item.objective_id.clone().expect("listed items are linked"),
            title: item.title.clone(),
            body: item.body.clone(),
            author: item.author.clone(),
            created_by_reili: item.author == REILI_LOGIN,
            state: item.state,
            assigned_to_reili: item.assigned_to_reili,
            needs_decision: item.needs_decision,
            created_at: item.created_at,
            updated_at: item.updated_at,
        }
    }

    fn reili_comment(&self, number: u64, body: &str) {
        let now = self.clock.now();
        self.update(number, |item| {
            item.comments.push(FakeComment {
                author: REILI_LOGIN.to_string(),
                by_reili: true,
                trusted: true,
                body: body.to_string(),
                created_at: now,
            });
        });
    }
}

#[async_trait]
impl WorkStatePort for FakeTracker {
    async fn list_work_items(
        &self,
        objective_id: &ObjectiveId,
        closed_since: DateTime<Utc>,
    ) -> Result<Vec<WorkItem>, WorkStateError> {
        self.check_available()?;
        Ok(self
            .state
            .lock()
            .expect("lock")
            .items
            .values()
            .filter(|item| item.objective_id.as_ref() == Some(objective_id))
            .filter(|item| {
                item.state == WorkItemState::Open
                    || item.closed_at.is_some_and(|closed| closed >= closed_since)
            })
            .map(Self::to_work_item)
            .collect())
    }

    async fn list_unlinked_reili_work(&self) -> Result<Vec<RecordRef>, WorkStateError> {
        self.check_available()?;
        Ok(self
            .state
            .lock()
            .expect("lock")
            .items
            .values()
            .filter(|item| {
                item.objective_id.is_none()
                    && item.assigned_to_reili
                    && item.state == WorkItemState::Open
            })
            .map(|item| Self::record(item.number))
            .collect())
    }

    async fn link_to_objective(
        &self,
        record: &RecordRef,
        objective_id: &ObjectiveId,
    ) -> Result<(), WorkStateError> {
        self.check_available()?;
        self.update(Self::number_of(record), |item| {
            item.objective_id = Some(objective_id.clone());
        });
        Ok(())
    }

    async fn list_comments(
        &self,
        record: &RecordRef,
    ) -> Result<Vec<WorkItemComment>, WorkStateError> {
        self.check_available()?;
        let item = self.item(Self::number_of(record));
        Ok(item
            .comments
            .iter()
            .map(|comment| WorkItemComment {
                author: comment.author.clone(),
                by_reili: comment.by_reili,
                author_trusted: comment.trusted,
                body: comment.body.clone(),
                created_at: comment.created_at,
                url: format!("{}#comment", record.url),
            })
            .collect())
    }

    async fn create_work_item(&self, new: &NewWorkItem) -> Result<WorkItem, WorkStateError> {
        self.check_available()?;
        let mut state = self.state.lock().expect("lock");
        let mut item = self.new_item(Some(&new.objective_id), &new.title, &new.body, REILI_LOGIN);
        item.assigned_to_reili = true;
        item.needs_decision = new.needs_decision;
        let number = self.insert(&mut state, item);
        Ok(Self::to_work_item(&state.items[&number]))
    }

    async fn add_comment(&self, record: &RecordRef, body: &str) -> Result<(), WorkStateError> {
        self.check_available()?;
        self.reili_comment(Self::number_of(record), body);
        Ok(())
    }

    async fn set_needs_decision(
        &self,
        record: &RecordRef,
        needs_decision: bool,
    ) -> Result<(), WorkStateError> {
        self.check_available()?;
        self.update(Self::number_of(record), |item| {
            item.needs_decision = needs_decision;
        });
        Ok(())
    }

    async fn close_work_item(&self, record: &RecordRef) -> Result<(), WorkStateError> {
        self.check_available()?;
        let now = self.clock.now();
        self.update(Self::number_of(record), |item| {
            item.state = WorkItemState::Done;
            item.closed_at = Some(now);
        });
        Ok(())
    }
}

/// Scripted cycle agent: returns queued reports in order and records each brief.
#[derive(Clone, Default)]
pub(crate) struct ScriptedCycleAgentRunner {
    responses: Arc<Mutex<Vec<ScriptedResponse>>>,
    pub briefs: Arc<Mutex<Vec<String>>>,
}

pub(crate) enum ScriptedResponse {
    Report {
        report: Option<CycleReport>,
        tokens: u64,
    },
    Fail(String),
    Hang,
    Panic,
}

impl ScriptedCycleAgentRunner {
    pub(crate) fn push(&self, response: ScriptedResponse) {
        self.responses.lock().expect("lock").push(response);
    }

    pub(crate) fn push_report(&self, report: CycleReport, tokens: u64) {
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
impl CycleAgentPort for ScriptedCycleAgentRunner {
    async fn run(
        &self,
        input: RunCycleAgentInput,
    ) -> Result<CycleAgentOutcome, AgentRunFailedError> {
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
            None => Ok(CycleAgentOutcome::Completed(Box::new(CycleAgentRun {
                report: Some(CycleReport {
                    slack_message: None,
                    evidence: Vec::new(),
                    work_items: Vec::new(),
                    knowledge_candidates: Vec::new(),
                    no_new_findings: true,
                }),
                usage: LlmUsageSnapshot::default(),
            }))),
            Some(ScriptedResponse::Report { report, tokens }) => {
                input.budget.record_tokens(tokens);
                Ok(CycleAgentOutcome::Completed(Box::new(CycleAgentRun {
                    report,
                    usage: LlmUsageSnapshot {
                        requests: 1,
                        input_tokens: tokens,
                        output_tokens: 0,
                        total_tokens: tokens,
                    },
                })))
            }
            Some(ScriptedResponse::Fail(message)) => Err(AgentRunFailedError::new(
                LlmUsageSnapshot::default(),
                message,
            )),
            Some(ScriptedResponse::Hang) => {
                input.cancellation.wait_for_cancellation().await;
                Ok(CycleAgentOutcome::Cancelled)
            }
            Some(ScriptedResponse::Panic) => panic!("scripted agent panic"),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct RecordingReportPort {
    pub posted: Arc<Mutex<Vec<SlackPostMessageInput>>>,
    pub fail_posts: Arc<Mutex<bool>>,
}

impl RecordingReportPort {
    pub(crate) fn posted(&self) -> Vec<SlackPostMessageInput> {
        self.posted.lock().expect("lock").clone()
    }
}

#[async_trait]
impl SlackPostMessagePort for RecordingReportPort {
    async fn post_message(&self, message: SlackPostMessageInput) -> Result<(), PortError> {
        if *self.fail_posts.lock().expect("lock") {
            return Err(PortError::connection_failed("slack down"));
        }
        self.posted.lock().expect("lock").push(message);
        Ok(())
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

struct FakeTrackerHarness(FakeTracker);

#[async_trait]
impl reili_core::work::conformance::WorkStateBackendHarness for FakeTrackerHarness {
    fn repository(&self) -> Arc<dyn WorkStatePort> {
        Arc::new(self.0.clone())
    }

    async fn create_as_human(&self, objective: &ObjectiveId, title: &str, body: &str) -> RecordRef {
        FakeTracker::record(self.0.human_create(objective.as_str(), title, body, false))
    }

    async fn create_unlinked_as_human(&self, title: &str, body: &str) -> RecordRef {
        FakeTracker::record(self.0.human_create_unlinked(title, body, false))
    }

    async fn comment_as_human(&self, record: &RecordRef, author: &str, trusted: bool, body: &str) {
        self.0
            .human_comment(FakeTracker::number_of(record), author, trusted, body);
    }

    async fn set_assigned_to_reili_as_human(&self, record: &RecordRef, assigned: bool) {
        self.0
            .human_assign(FakeTracker::number_of(record), assigned);
    }
}

#[tokio::test]
async fn fake_tracker_satisfies_the_backend_contract() {
    let harness = FakeTrackerHarness(FakeTracker::new(ManualClock::new(at(
        "2026-10-15T00:00:00Z",
    ))));

    reili_core::work::conformance::assert_work_state_contract(&harness).await;
}
