//! `WorkStateRepositoryPort` on GitHub Issues of a single management repository.
//!
//! - Reads use the issue *listing* API filtered by labels (strongly consistent), never search,
//!   which lags behind writes.
//! - Writes are read-modify-write with a revision number stored in the issue body; GitHub has no
//!   conditional updates, so a narrow race window remains and is documented, not hidden.
//! - Every request targets the configured repository; issue references pointing anywhere else
//!   are refused before any network call.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reili_core::error::PortError;
use reili_core::objective::ObjectiveId;
use reili_core::secret::SecretString;
use reili_core::work::{
    HumanComment, ObjectiveLedgerState, RecordRef, StoredLedger, StoredWorkItem, WorkItemState,
    WorkItemStatus, WorkStateComment, WorkStateError, WorkStateRepositoryPort,
    sanitize::truncate_chars,
};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::codec::{
    COMMENT_MARKER_PREFIX, decode_state, extract_human_notes, render_comment, render_ledger_body,
    render_work_item_body,
};
use crate::outbound::github::app_token::{GitHubAppTokenProvider, InstallationTokenScope};

const PER_PAGE: usize = 100;
const MAX_PAGES: usize = 20;
const API_VERSION: &str = "2022-11-28";
const USER_AGENT: &str = "reili-work-state";
pub(super) const OBJECTIVE_LEDGER_LABEL: &str = "reili:objective";
pub(super) const WORK_ITEM_LABEL: &str = "reili:work-item";
const DUPLICATE_LABEL: &str = "reili:duplicate";
const MANAGED_LABEL_PREFIXES: &[&str] = &[
    "reili:",
    "reili-objective:",
    "reili-status:",
    "reili-severity:",
    "reili-kind:",
];
const TRUSTED_ASSOCIATIONS: &[&str] = &["OWNER", "MEMBER", "COLLABORATOR"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubWorkStateConfig {
    /// `owner/name` of the dedicated management repository.
    pub repository: String,
    pub app_id: String,
    pub private_key: SecretString,
    pub installation_id: u32,
    pub api_base_url: String,
    pub web_base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagementRepository {
    owner: String,
    name: String,
}

impl ManagementRepository {
    fn parse(value: &str) -> Result<Self, PortError> {
        let valid = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        };
        match value.trim().split_once('/') {
            Some((owner, name)) if valid(owner) && valid(name) => Ok(Self {
                owner: owner.to_string(),
                name: name.to_string(),
            }),
            _ => Err(PortError::invalid_input(format!(
                "work state repository must be `owner/name`, got `{value}`"
            ))),
        }
    }
}

#[async_trait]
pub(crate) trait AccessTokenSource: Send + Sync {
    async fn token(&self) -> Result<SecretString, PortError>;
}

#[async_trait]
impl AccessTokenSource for GitHubAppTokenProvider {
    async fn token(&self) -> Result<SecretString, PortError> {
        GitHubAppTokenProvider::token(self).await
    }
}

#[cfg(any(test, feature = "test-support"))]
struct StaticAccessToken(SecretString);

#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl AccessTokenSource for StaticAccessToken {
    async fn token(&self) -> Result<SecretString, PortError> {
        Ok(self.0.clone())
    }
}

pub struct GitHubWorkStateRepository {
    http: reqwest::Client,
    token: Arc<dyn AccessTokenSource>,
    repository: ManagementRepository,
    api_base_url: String,
    web_base_url: String,
    known_labels: Mutex<BTreeSet<String>>,
}

#[derive(Debug, Deserialize)]
struct ApiLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ApiIssue {
    number: u64,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    #[serde(default)]
    state_reason: Option<String>,
    #[serde(default)]
    labels: Vec<ApiLabel>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    pull_request: Option<Value>,
}

impl ApiIssue {
    fn label_names(&self) -> Vec<&str> {
        self.labels
            .iter()
            .map(|label| label.name.as_str())
            .collect()
    }

    fn record_ref(&self) -> RecordRef {
        RecordRef {
            key: record_key(self.number),
            url: self.html_url.clone(),
        }
    }

    fn is_duplicate(&self) -> bool {
        self.label_names().contains(&DUPLICATE_LABEL)
    }
}

#[derive(Debug, Deserialize)]
struct ApiUser {
    login: String,
    #[serde(rename = "type", default)]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct ApiComment {
    id: u64,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    user: Option<ApiUser>,
    #[serde(default)]
    author_association: String,
    created_at: DateTime<Utc>,
    html_url: String,
}

/// GitHub records are keyed by issue number in the familiar `#<number>` form.
fn record_key(number: u64) -> String {
    format!("#{number}")
}

fn parse_record_key(key: &str) -> Option<u64> {
    key.strip_prefix('#')?.parse().ok()
}

fn objective_label(objective_id: &str) -> String {
    format!("reili-objective:{objective_id}")
}

fn status_label(status: WorkItemStatus) -> String {
    format!("reili-status:{}", status.as_str())
}

fn is_managed_label(name: &str) -> bool {
    MANAGED_LABEL_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Status implied by labels and open/closed state, so human edits on GitHub are visible.
fn observed_status(issue: &ApiIssue, recorded: WorkItemStatus) -> WorkItemStatus {
    let label = issue
        .labels
        .iter()
        .find_map(|label| label.name.strip_prefix("reili-status:"))
        .and_then(WorkItemStatus::parse);
    if issue.state == "closed" {
        match label {
            Some(status) if !status.is_open() => status,
            _ if issue.state_reason.as_deref() == Some("not_planned") => WorkItemStatus::Dismissed,
            _ => WorkItemStatus::Resolved,
        }
    } else {
        match label {
            Some(status) if status.is_open() => status,
            Some(_) => WorkItemStatus::Investigating,
            None if recorded.is_open() => recorded,
            None => WorkItemStatus::Investigating,
        }
    }
}

fn work_item_labels(state: &WorkItemState) -> Vec<String> {
    vec![
        WORK_ITEM_LABEL.to_string(),
        objective_label(state.objective_id.as_str()),
        status_label(state.status),
        format!("reili-severity:{}", state.severity.as_str()),
        format!("reili-kind:{}", state.kind.as_str()),
    ]
}

fn ledger_labels(state: &ObjectiveLedgerState) -> Vec<String> {
    vec![
        OBJECTIVE_LEDGER_LABEL.to_string(),
        objective_label(state.objective_id.as_str()),
    ]
}

fn label_color(name: &str) -> &'static str {
    if name.starts_with("reili-status:needs-human") {
        "d93f0b"
    } else if name.starts_with("reili-status:") {
        "0e8a16"
    } else if name.starts_with("reili-severity:") {
        "fbca04"
    } else {
        "5319e7"
    }
}

fn work_item_title(state: &WorkItemState) -> String {
    truncate_chars(&format!("[Reili] {}", state.title.trim()), 200)
}

fn ledger_title(state: &ObjectiveLedgerState) -> String {
    let description = state.description.lines().next().unwrap_or_default().trim();
    truncate_chars(
        &format!("[Reili objective] {}: {description}", state.objective_id),
        200,
    )
}

fn map_status_error(status: u16, body: &str, context: &str) -> WorkStateError {
    let message = format!(
        "{context}: GitHub returned {status}: {}",
        truncate_chars(body, 300)
    );
    let rate_limited =
        status == 429 || (status == 403 && body.to_lowercase().contains("rate limit"));
    if status >= 500 || rate_limited {
        WorkStateError::Unavailable(PortError::http_status(status, message))
    } else if status == 403 {
        WorkStateError::Forbidden(message)
    } else {
        WorkStateError::Other(PortError::http_status(status, message))
    }
}

impl GitHubWorkStateRepository {
    pub fn new(config: &GitHubWorkStateConfig) -> Result<Self, PortError> {
        let repository = ManagementRepository::parse(&config.repository)?;
        let token = GitHubAppTokenProvider::new(
            &config.app_id,
            &config.private_key,
            config.installation_id,
            &config.api_base_url,
            InstallationTokenScope::work_state(&repository.name),
            USER_AGENT,
        )?;
        Self::with_token_source(config, Arc::new(token))
    }

    /// Test-only constructor using a fixed token (no App JWT signing).
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_static_token(
        config: &GitHubWorkStateConfig,
        token: SecretString,
    ) -> Result<Self, PortError> {
        Self::with_token_source(config, Arc::new(StaticAccessToken(token)))
    }

    pub(crate) fn with_token_source(
        config: &GitHubWorkStateConfig,
        token: Arc<dyn AccessTokenSource>,
    ) -> Result<Self, PortError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| PortError::new(format!("Failed to build GitHub client: {error}")))?;
        Ok(Self {
            http,
            token,
            repository: ManagementRepository::parse(&config.repository)?,
            api_base_url: config.api_base_url.trim_end_matches('/').to_string(),
            web_base_url: config.web_base_url.trim_end_matches('/').to_string(),
            known_labels: Mutex::new(BTreeSet::new()),
        })
    }

    #[must_use]
    pub fn repository_name(&self) -> String {
        format!("{}/{}", self.repository.owner, self.repository.name)
    }

    fn repo_url(&self) -> String {
        format!(
            "{}/repos/{}/{}",
            self.api_base_url, self.repository.owner, self.repository.name
        )
    }

    /// Resolves a record to its issue number, refusing references outside the management
    /// repository (defense in depth: request URLs are always built from config, but a foreign
    /// reference indicates a bug or tampering).
    fn guard(&self, record: &RecordRef) -> Result<u64, WorkStateError> {
        let expected_prefix = format!(
            "{}/{}/{}/issues/",
            self.web_base_url, self.repository.owner, self.repository.name
        )
        .to_lowercase();
        let url = record.url.to_lowercase();
        let number = parse_record_key(&record.key).filter(|number| {
            url.starts_with(&expected_prefix)
                && url[expected_prefix.len()..]
                    .split(['#', '?'])
                    .next()
                    .is_some_and(|in_url| in_url == number.to_string())
        });
        number.ok_or_else(|| {
            WorkStateError::Forbidden(format!(
                "refusing to touch {} ({}) — only issues in {} are writable",
                record.key,
                record.url,
                self.repository_name()
            ))
        })
    }

    async fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
        context: &str,
    ) -> Result<(u16, String), WorkStateError> {
        let token = self.token.token().await.map_err(|error| {
            if error.is_connection_failed() || error.status_code().is_some_and(|code| code >= 500) {
                WorkStateError::Unavailable(error)
            } else {
                WorkStateError::Other(error)
            }
        })?;
        let auth =
            HeaderValue::from_str(&format!("Bearer {}", token.expose())).map_err(|error| {
                WorkStateError::Other(PortError::invalid_input(format!(
                    "invalid token header: {error}"
                )))
            })?;
        let mut builder = self
            .http
            .request(method, url)
            .header(AUTHORIZATION, auth)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION);
        if let Some(body) = body {
            builder = builder.json(body);
        }
        let response = builder.send().await.map_err(|error| {
            WorkStateError::Unavailable(PortError::connection_failed(format!(
                "{context}: GitHub request failed: {error}"
            )))
        })?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|error| {
            WorkStateError::Unavailable(PortError::invalid_response(format!(
                "{context}: failed to read GitHub response: {error}"
            )))
        })?;
        Ok((status, text))
    }

    async fn request_json<T: for<'de> Deserialize<'de>>(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
        context: &str,
    ) -> Result<T, WorkStateError> {
        let (status, text) = self.request(method, url, body, context).await?;
        if !(200..300).contains(&status) {
            return Err(map_status_error(status, &text, context));
        }
        serde_json::from_str(&text).map_err(|error| {
            WorkStateError::Other(PortError::invalid_response(format!(
                "{context}: unexpected GitHub response: {error}"
            )))
        })
    }

    async fn list_issues(&self, labels: &[String]) -> Result<Vec<ApiIssue>, WorkStateError> {
        let mut issues = Vec::new();
        for page in 1..=MAX_PAGES {
            let query = serde_urlencoded::to_string([
                ("state", "all".to_string()),
                ("labels", labels.join(",")),
                ("per_page", PER_PAGE.to_string()),
                ("page", page.to_string()),
                ("sort", "created".to_string()),
                ("direction", "asc".to_string()),
            ])
            .unwrap_or_default();
            let batch: Vec<ApiIssue> = self
                .request_json(
                    reqwest::Method::GET,
                    &format!("{}/issues?{query}", self.repo_url()),
                    None,
                    "list issues",
                )
                .await?;
            let count = batch.len();
            issues.extend(
                batch
                    .into_iter()
                    .filter(|issue| issue.pull_request.is_none() && !issue.is_duplicate()),
            );
            if count < PER_PAGE {
                break;
            }
        }
        issues.sort_by_key(|issue| issue.number);
        Ok(issues)
    }

    async fn get_issue(&self, number: u64) -> Result<ApiIssue, WorkStateError> {
        self.request_json(
            reqwest::Method::GET,
            &format!("{}/issues/{number}", self.repo_url()),
            None,
            "get issue",
        )
        .await
    }

    async fn ensure_labels(&self, labels: &[String]) -> Result<(), WorkStateError> {
        let missing: Vec<String> = {
            let known = self.known_labels.lock().await;
            labels
                .iter()
                .filter(|label| !known.contains(*label))
                .cloned()
                .collect()
        };
        for label in missing {
            let (status, text) = self
                .request(
                    reqwest::Method::POST,
                    &format!("{}/labels", self.repo_url()),
                    Some(&json!({
                        "name": label,
                        "color": label_color(&label),
                        "description": "Managed by Reili",
                    })),
                    "create label",
                )
                .await?;
            // 422 means the label already exists.
            if !(200..300).contains(&status) && status != 422 {
                return Err(map_status_error(status, &text, "create label"));
            }
            self.known_labels.lock().await.insert(label);
        }
        Ok(())
    }

    fn to_ledger(issue: &ApiIssue) -> Result<StoredLedger, WorkStateError> {
        let body = issue.body.as_deref().unwrap_or_default();
        Ok(StoredLedger {
            record: issue.record_ref(),
            state: decode_state(body).map_err(|message| WorkStateError::Corrupted {
                record: record_key(issue.number),
                message,
            })?,
            human_notes: extract_human_notes(body),
        })
    }

    fn to_work_item(issue: &ApiIssue) -> Result<StoredWorkItem, WorkStateError> {
        let body = issue.body.as_deref().unwrap_or_default();
        let state: WorkItemState =
            decode_state(body).map_err(|message| WorkStateError::Corrupted {
                record: record_key(issue.number),
                message,
            })?;
        Ok(StoredWorkItem {
            record: issue.record_ref(),
            observed_status: observed_status(issue, state.status),
            state,
            human_notes: extract_human_notes(body),
            remote_updated_at: issue.updated_at,
        })
    }

    fn merged_labels(issue: &ApiIssue, managed: Vec<String>) -> Vec<String> {
        let mut labels: Vec<String> = issue
            .labels
            .iter()
            .map(|label| label.name.clone())
            .filter(|name| !is_managed_label(name))
            .collect();
        labels.extend(managed);
        labels
    }
}

fn state_fields(status: WorkItemStatus) -> Value {
    match status {
        WorkItemStatus::Resolved => json!({ "state": "closed", "state_reason": "completed" }),
        WorkItemStatus::Dismissed => json!({ "state": "closed", "state_reason": "not_planned" }),
        _ => json!({ "state": "open" }),
    }
}

fn corrupted_skip(error: &WorkStateError) -> bool {
    matches!(error, WorkStateError::Corrupted { .. })
}

#[async_trait]
impl WorkStateRepositoryPort for GitHubWorkStateRepository {
    async fn find_objective_ledger(
        &self,
        objective_id: &ObjectiveId,
    ) -> Result<Option<StoredLedger>, WorkStateError> {
        let issues = self
            .list_issues(&[
                OBJECTIVE_LEDGER_LABEL.to_string(),
                objective_label(objective_id.as_str()),
            ])
            .await?;
        issues
            .iter()
            .min_by_key(|issue| issue.number)
            .map(Self::to_ledger)
            .transpose()
    }

    async fn list_objective_ledgers(&self) -> Result<Vec<StoredLedger>, WorkStateError> {
        let issues = self
            .list_issues(&[OBJECTIVE_LEDGER_LABEL.to_string()])
            .await?;
        let mut ledgers = Vec::new();
        for issue in &issues {
            match Self::to_ledger(issue) {
                Ok(ledger) => ledgers.push(ledger),
                Err(error) if corrupted_skip(&error) => {
                    tracing::warn!(record = %record_key(issue.number), error = %error, "Skipping unreadable objective ledger");
                }
                Err(error) => return Err(error),
            }
        }
        Ok(ledgers)
    }

    async fn create_objective_ledger(
        &self,
        state: &ObjectiveLedgerState,
    ) -> Result<StoredLedger, WorkStateError> {
        let mut state = state.clone();
        state.revision = 1;
        let labels = ledger_labels(&state);
        self.ensure_labels(&labels).await?;
        let body = render_ledger_body(&state, "")
            .map_err(|message| WorkStateError::Other(PortError::invalid_input(message)))?;
        let issue: ApiIssue = self
            .request_json(
                reqwest::Method::POST,
                &format!("{}/issues", self.repo_url()),
                Some(&json!({ "title": ledger_title(&state), "body": body, "labels": labels })),
                "create objective ledger",
            )
            .await?;
        Self::to_ledger(&issue)
    }

    async fn save_objective_ledger(
        &self,
        record: &RecordRef,
        expected_revision: u64,
        state: &ObjectiveLedgerState,
    ) -> Result<StoredLedger, WorkStateError> {
        let number = self.guard(record)?;
        let current = self.get_issue(number).await?;
        let stored = Self::to_ledger(&current)?;
        if stored.state.revision != expected_revision {
            return Err(WorkStateError::Conflict {
                record: record.key.clone(),
                expected: expected_revision,
                found: stored.state.revision,
            });
        }
        let mut next = state.clone();
        next.revision = expected_revision + 1;
        let labels = Self::merged_labels(&current, ledger_labels(&next));
        let body = render_ledger_body(&next, &stored.human_notes)
            .map_err(|message| WorkStateError::Other(PortError::invalid_input(message)))?;
        let updated: ApiIssue = self
            .request_json(
                reqwest::Method::PATCH,
                &format!("{}/issues/{number}", self.repo_url()),
                Some(&json!({ "title": ledger_title(&next), "body": body, "labels": labels })),
                "update objective ledger",
            )
            .await?;
        Self::to_ledger(&updated)
    }

    async fn list_work_items(
        &self,
        objective_id: &ObjectiveId,
    ) -> Result<Vec<StoredWorkItem>, WorkStateError> {
        let issues = self
            .list_issues(&[
                WORK_ITEM_LABEL.to_string(),
                objective_label(objective_id.as_str()),
            ])
            .await?;
        let mut items = Vec::new();
        for issue in &issues {
            match Self::to_work_item(issue) {
                Ok(item) => items.push(item),
                Err(error) if corrupted_skip(&error) => {
                    tracing::warn!(record = %record_key(issue.number), error = %error, "Skipping unreadable work item");
                }
                Err(error) => return Err(error),
            }
        }
        Ok(items)
    }

    async fn create_work_item(
        &self,
        state: &WorkItemState,
    ) -> Result<StoredWorkItem, WorkStateError> {
        let mut state = state.clone();
        state.revision = 1;
        let labels = work_item_labels(&state);
        self.ensure_labels(&labels).await?;
        let body = render_work_item_body(&state, "")
            .map_err(|message| WorkStateError::Other(PortError::invalid_input(message)))?;
        let issue: ApiIssue = self
            .request_json(
                reqwest::Method::POST,
                &format!("{}/issues", self.repo_url()),
                Some(&json!({ "title": work_item_title(&state), "body": body, "labels": labels })),
                "create work item",
            )
            .await?;
        if !state.status.is_open() {
            let closed: ApiIssue = self
                .request_json(
                    reqwest::Method::PATCH,
                    &format!("{}/issues/{}", self.repo_url(), issue.number),
                    Some(&state_fields(state.status)),
                    "close work item",
                )
                .await?;
            return Self::to_work_item(&closed);
        }
        Self::to_work_item(&issue)
    }

    async fn save_work_item(
        &self,
        record: &RecordRef,
        expected_revision: u64,
        state: &WorkItemState,
    ) -> Result<StoredWorkItem, WorkStateError> {
        let number = self.guard(record)?;
        let current = self.get_issue(number).await?;
        let stored = Self::to_work_item(&current)?;
        if stored.state.revision != expected_revision {
            return Err(WorkStateError::Conflict {
                record: record.key.clone(),
                expected: expected_revision,
                found: stored.state.revision,
            });
        }
        let mut next = state.clone();
        next.revision = expected_revision + 1;
        let managed = work_item_labels(&next);
        self.ensure_labels(&managed).await?;
        let body = render_work_item_body(&next, &stored.human_notes)
            .map_err(|message| WorkStateError::Other(PortError::invalid_input(message)))?;
        let mut patch = json!({
            "title": work_item_title(&next),
            "body": body,
            "labels": Self::merged_labels(&current, managed),
        });
        if let (Some(target), Some(fields)) =
            (patch.as_object_mut(), state_fields(next.status).as_object())
        {
            for (key, value) in fields {
                target.insert(key.clone(), value.clone());
            }
        }
        let updated: ApiIssue = self
            .request_json(
                reqwest::Method::PATCH,
                &format!("{}/issues/{number}", self.repo_url()),
                Some(&patch),
                "update work item",
            )
            .await?;
        Self::to_work_item(&updated)
    }

    async fn append_comment(
        &self,
        record: &RecordRef,
        comment: WorkStateComment,
    ) -> Result<(), WorkStateError> {
        let number = self.guard(record)?;
        let _: Value = self
            .request_json(
                reqwest::Method::POST,
                &format!("{}/issues/{number}/comments", self.repo_url()),
                Some(&json!({ "body": render_comment(comment.kind.as_str(), &comment.body) })),
                "create comment",
            )
            .await?;
        Ok(())
    }

    async fn list_human_comments(
        &self,
        record: &RecordRef,
        after_comment_id: Option<String>,
    ) -> Result<Vec<HumanComment>, WorkStateError> {
        let number = self.guard(record)?;
        let after_comment_id = after_comment_id
            .map(|cursor| {
                cursor
                    .parse::<u64>()
                    .map_err(|_| WorkStateError::Corrupted {
                        record: record.key.clone(),
                        message: format!("comment cursor `{cursor}` is not a GitHub comment id"),
                    })
            })
            .transpose()?;
        let mut comments = Vec::new();
        for page in 1..=MAX_PAGES {
            let batch: Vec<ApiComment> = self
                .request_json(
                    reqwest::Method::GET,
                    &format!(
                        "{}/issues/{number}/comments?per_page={PER_PAGE}&page={page}",
                        self.repo_url(),
                    ),
                    None,
                    "list comments",
                )
                .await?;
            let count = batch.len();
            for comment in batch {
                if after_comment_id.is_some_and(|after| comment.id <= after) {
                    continue;
                }
                let body = comment.body.unwrap_or_default();
                let user = comment.user.unwrap_or(ApiUser {
                    login: "ghost".to_string(),
                    kind: String::new(),
                });
                let by_reili = user.kind == "Bot" && body.starts_with(COMMENT_MARKER_PREFIX);
                if by_reili {
                    continue;
                }
                comments.push((
                    comment.id,
                    HumanComment {
                        id: comment.id.to_string(),
                        author_trusted: user.kind != "Bot"
                            && TRUSTED_ASSOCIATIONS.contains(&comment.author_association.as_str()),
                        author: user.login,
                        body,
                        created_at: comment.created_at,
                        url: comment.html_url,
                    },
                ));
            }
            if count < PER_PAGE {
                break;
            }
        }
        comments.sort_by_key(|(id, _)| *id);
        Ok(comments.into_iter().map(|(_, comment)| comment).collect())
    }

    async fn close_duplicate(
        &self,
        duplicate: &RecordRef,
        canonical: &RecordRef,
    ) -> Result<(), WorkStateError> {
        let duplicate_number = self.guard(duplicate)?;
        self.guard(canonical)?;
        self.ensure_labels(&[DUPLICATE_LABEL.to_string()]).await?;
        self.append_comment(
            duplicate,
            WorkStateComment {
                kind: reili_core::work::WorkStateCommentKind::Notice,
                body: format!(
                    "Duplicate of {} (created concurrently); Reili continues on {}.",
                    canonical.key, canonical.key
                ),
            },
        )
        .await?;
        let _: ApiIssue = self
            .request_json(
                reqwest::Method::PATCH,
                &format!("{}/issues/{duplicate_number}", self.repo_url()),
                Some(&json!({
                    "state": "closed",
                    "state_reason": "not_planned",
                    "labels": [DUPLICATE_LABEL],
                })),
                "close duplicate",
            )
            .await?;
        Ok(())
    }
}
