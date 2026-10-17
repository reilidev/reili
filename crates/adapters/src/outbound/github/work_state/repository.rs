//! `WorkStatePort` on GitHub Issues of a single, explicitly configured repository — the one the
//! team already uses for its work.
//!
//! - An objective's items carry `reili-objective:<id>`; Reili's own work carries `reili:assignee`
//!   (labels stand in for assignees, which GitHub Apps cannot reliably be); `reili:needs-decision`
//!   marks items waiting for the team.
//! - Reads use the issue *listing* API filtered by labels (strongly consistent), never search,
//!   which lags behind writes.
//! - Label changes use the add/remove endpoints, so labels the team sets are never overwritten.
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
    CloseReason, NewWorkItem, RecordKey, RecordRef, WorkItem, WorkItemComment, WorkItemState,
    WorkStateError, WorkStatePort, sanitize::truncate_chars,
};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::codec::{
    COMMENT_MARKER, parse_issue_body, render_comment, render_issue_body, strip_comment_marker,
};
use crate::outbound::github::app_token::{GitHubAppTokenProvider, InstallationTokenScope};

const PER_PAGE: usize = 100;
const MAX_PAGES: usize = 20;
const API_VERSION: &str = "2022-11-28";
const USER_AGENT: &str = "reili-work-state";
pub(super) const ASSIGNEE_LABEL: &str = "reili:assignee";
pub(super) const NEEDS_DECISION_LABEL: &str = "reili:needs-decision";
const TRUSTED_ASSOCIATIONS: &[&str] = &["OWNER", "MEMBER", "COLLABORATOR"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubWorkStateConfig {
    /// `owner/name` of the repository the team tracks its work in.
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
struct ApiUser {
    login: String,
    #[serde(rename = "type", default)]
    kind: String,
}

impl ApiUser {
    fn is_bot(&self) -> bool {
        self.kind == "Bot"
    }
}

#[derive(Debug, Deserialize)]
struct ApiIssue {
    number: u64,
    html_url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    user: Option<ApiUser>,
    state: String,
    #[serde(default)]
    state_reason: Option<String>,
    #[serde(default)]
    labels: Vec<ApiLabel>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    closed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pull_request: Option<Value>,
}

impl ApiIssue {
    fn has_label(&self, name: &str) -> bool {
        self.labels.iter().any(|label| label.name == name)
    }

    fn work_state(&self) -> WorkItemState {
        match (self.state.as_str(), self.state_reason.as_deref()) {
            ("open", _) => WorkItemState::Open,
            (_, Some("not_planned" | "duplicate")) => WorkItemState::NotPlanned,
            _ => WorkItemState::Done,
        }
    }

    fn to_work_item(&self, objective_id: &ObjectiveId) -> WorkItem {
        let (body, key) = parse_issue_body(self.body.as_deref().unwrap_or_default());
        let created_by_reili = self.user.as_ref().is_some_and(ApiUser::is_bot) && key.is_some();
        WorkItem {
            record: RecordRef {
                key: record_key(self.number),
                url: self.html_url.clone(),
            },
            objective_id: objective_id.clone(),
            title: self.title.clone(),
            body,
            author: self
                .user
                .as_ref()
                .map_or_else(|| "ghost".to_string(), |user| user.login.clone()),
            created_by_reili,
            // A key copied into someone else's issue is not Reili's metadata.
            key: key.filter(|_| created_by_reili),
            state: self.work_state(),
            assigned_to_reili: self.has_label(ASSIGNEE_LABEL),
            needs_decision: self.has_label(NEEDS_DECISION_LABEL),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiComment {
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
fn record_key(number: u64) -> RecordKey {
    RecordKey::new(format!("#{number}"))
}

fn parse_record_key(key: &RecordKey) -> Option<u64> {
    key.as_str().strip_prefix('#')?.parse().ok()
}

fn objective_label(objective_id: &ObjectiveId) -> String {
    format!("reili-objective:{objective_id}")
}

fn label_color(name: &str) -> &'static str {
    match name {
        NEEDS_DECISION_LABEL => "d93f0b",
        ASSIGNEE_LABEL => "0e8a16",
        _ => "5319e7",
    }
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

    async fn list_issues(
        &self,
        labels: &[String],
        state: &str,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ApiIssue>, WorkStateError> {
        let mut issues = Vec::new();
        for page in 1..=MAX_PAGES {
            let mut query = vec![
                ("state", state.to_string()),
                ("labels", labels.join(",")),
                ("per_page", PER_PAGE.to_string()),
                ("page", page.to_string()),
                ("sort", "created".to_string()),
                ("direction", "asc".to_string()),
            ];
            if let Some(since) = since {
                query.push(("since", since.to_rfc3339()));
            }
            let query = serde_urlencoded::to_string(query).unwrap_or_default();
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
                    .filter(|issue| issue.pull_request.is_none()),
            );
            if count < PER_PAGE {
                break;
            }
        }
        Ok(issues)
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
}

#[async_trait]
impl WorkStatePort for GitHubWorkStateRepository {
    async fn list_work_items(
        &self,
        objective_id: &ObjectiveId,
        closed_since: DateTime<Utc>,
    ) -> Result<Vec<WorkItem>, WorkStateError> {
        let labels = [objective_label(objective_id)];
        let mut issues = self.list_issues(&labels, "open", None).await?;
        issues.extend(
            self.list_issues(&labels, "closed", Some(closed_since))
                .await?
                .into_iter()
                .filter(|issue| issue.closed_at.is_some_and(|closed| closed >= closed_since)),
        );
        issues.sort_by_key(|issue| issue.number);
        issues.dedup_by_key(|issue| issue.number);
        Ok(issues
            .iter()
            .map(|issue| issue.to_work_item(objective_id))
            .collect())
    }

    async fn list_comments(
        &self,
        record: &RecordRef,
    ) -> Result<Vec<WorkItemComment>, WorkStateError> {
        let number = self.guard(record)?;
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
                let body = comment.body.unwrap_or_default();
                let user = comment.user.unwrap_or(ApiUser {
                    login: "ghost".to_string(),
                    kind: String::new(),
                });
                let by_reili = user.is_bot() && body.starts_with(COMMENT_MARKER);
                comments.push(WorkItemComment {
                    author_trusted: by_reili
                        || (!user.is_bot()
                            && TRUSTED_ASSOCIATIONS.contains(&comment.author_association.as_str())),
                    by_reili,
                    author: user.login,
                    body: if by_reili {
                        strip_comment_marker(&body)
                    } else {
                        body
                    },
                    created_at: comment.created_at,
                    url: comment.html_url,
                });
            }
            if count < PER_PAGE {
                break;
            }
        }
        Ok(comments)
    }

    async fn create_work_item(&self, item: &NewWorkItem) -> Result<WorkItem, WorkStateError> {
        let mut labels = vec![
            objective_label(&item.objective_id),
            ASSIGNEE_LABEL.to_string(),
        ];
        if item.needs_decision {
            labels.push(NEEDS_DECISION_LABEL.to_string());
        }
        self.ensure_labels(&labels).await?;
        let issue: ApiIssue = self
            .request_json(
                reqwest::Method::POST,
                &format!("{}/issues", self.repo_url()),
                Some(&json!({
                    "title": truncate_chars(item.title.trim(), 200),
                    "body": render_issue_body(&item.body, &item.key),
                    "labels": labels,
                })),
                "create work item",
            )
            .await?;
        Ok(issue.to_work_item(&item.objective_id))
    }

    async fn add_comment(&self, record: &RecordRef, body: &str) -> Result<(), WorkStateError> {
        let number = self.guard(record)?;
        let _: Value = self
            .request_json(
                reqwest::Method::POST,
                &format!("{}/issues/{number}/comments", self.repo_url()),
                Some(&json!({ "body": render_comment(body) })),
                "create comment",
            )
            .await?;
        Ok(())
    }

    async fn set_needs_decision(
        &self,
        record: &RecordRef,
        needs_decision: bool,
    ) -> Result<(), WorkStateError> {
        let number = self.guard(record)?;
        if needs_decision {
            self.ensure_labels(&[NEEDS_DECISION_LABEL.to_string()])
                .await?;
            let _: Value = self
                .request_json(
                    reqwest::Method::POST,
                    &format!("{}/issues/{number}/labels", self.repo_url()),
                    Some(&json!({ "labels": [NEEDS_DECISION_LABEL] })),
                    "add label",
                )
                .await?;
            return Ok(());
        }
        let label = NEEDS_DECISION_LABEL.replace(':', "%3A");
        let (status, text) = self
            .request(
                reqwest::Method::DELETE,
                &format!("{}/issues/{number}/labels/{label}", self.repo_url()),
                None,
                "remove label",
            )
            .await?;
        // 404 means the label was not on the issue.
        if !(200..300).contains(&status) && status != 404 {
            return Err(map_status_error(status, &text, "remove label"));
        }
        Ok(())
    }

    async fn close_work_item(
        &self,
        record: &RecordRef,
        reason: &CloseReason,
    ) -> Result<(), WorkStateError> {
        let number = self.guard(record)?;
        let state_reason = match reason {
            CloseReason::Done => "completed",
            CloseReason::DuplicateOf(canonical) => {
                self.guard(canonical)?;
                self.add_comment(
                    record,
                    &format!(
                        "Duplicate of {} (opened at the same time); Reili continues there.",
                        canonical.key
                    ),
                )
                .await?;
                "not_planned"
            }
        };
        let _: Value = self
            .request_json(
                reqwest::Method::PATCH,
                &format!("{}/issues/{number}", self.repo_url()),
                Some(&json!({ "state": "closed", "state_reason": state_reason })),
                "close work item",
            )
            .await?;
        Ok(())
    }
}
