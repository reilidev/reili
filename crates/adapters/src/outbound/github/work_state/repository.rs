use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reili_core::error::PortError;
use reili_core::objective::ObjectiveId;
use reili_core::secret::SecretString;
use reili_core::work::{
    NewWorkItem, RecordKey, RecordRef, RepositoryName, WorkItem, WorkItemComment, WorkItemState,
    WorkStateError, WorkStatePort, text::truncate_chars,
};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::outbound::github::app_token::{GitHubAppTokenProvider, InstallationTokenScope};

const PER_PAGE: usize = 100;
const MAX_BODY_CHARS: usize = 60_000;
const MAX_PAGES: usize = 20;
const API_VERSION: &str = "2022-11-28";
const USER_AGENT: &str = "reili-work-state";
pub(super) const ASSIGNEE_LABEL: &str = "reili:assignee";
pub(super) const NEEDS_DECISION_LABEL: &str = "reili:needs-decision";
pub(super) const OBJECTIVE_LABEL_PREFIX: &str = "reili-objective:";
const TRUSTED_ASSOCIATIONS: &[&str] = &["OWNER", "MEMBER", "COLLABORATOR"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubWorkStateConfig {
    /// `owner/name` of the repository the team tracks its work in.
    pub repository: RepositoryName,
    pub app_id: String,
    pub private_key: SecretString,
    pub installation_id: u32,
    pub api_base_url: String,
    pub web_base_url: String,
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

pub struct GitHubWorkStateRepository {
    http: reqwest::Client,
    app_id: u64,
    token: Arc<dyn AccessTokenSource>,
    repository: RepositoryName,
    api_base_url: String,
    web_base_url: String,
    known_labels: Mutex<BTreeSet<String>>,
}

#[derive(Debug, Deserialize)]
struct ApiLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ApiApp {
    id: u64,
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
    #[serde(default)]
    performed_via_github_app: Option<ApiApp>,
}

impl ApiIssue {
    fn has_label(&self, name: &str) -> bool {
        self.labels.iter().any(|label| label.name == name)
    }

    fn has_objective_label(&self) -> bool {
        self.labels
            .iter()
            .any(|label| label.name.starts_with(OBJECTIVE_LABEL_PREFIX))
    }

    fn record(&self) -> RecordRef {
        RecordRef {
            key: record_key(self.number),
            url: self.html_url.clone(),
        }
    }

    fn work_state(&self) -> WorkItemState {
        match (self.state.as_str(), self.state_reason.as_deref()) {
            ("open", _) => WorkItemState::Open,
            (_, Some("not_planned" | "duplicate")) => WorkItemState::NotPlanned,
            _ => WorkItemState::Done,
        }
    }

    fn to_work_item(&self, objective_id: &ObjectiveId, app_id: u64) -> WorkItem {
        WorkItem {
            record: self.record(),
            objective_id: objective_id.clone(),
            title: self.title.clone(),
            body: self.body.clone().unwrap_or_default(),
            author: self
                .user
                .as_ref()
                .map_or_else(|| "ghost".to_string(), |user| user.login.clone()),
            created_by_reili: performed_by(self.performed_via_github_app.as_ref(), app_id),
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
    #[serde(default)]
    performed_via_github_app: Option<ApiApp>,
}

fn performed_by(app: Option<&ApiApp>, app_id: u64) -> bool {
    app.is_some_and(|app| app.id == app_id)
}

fn record_key(number: u64) -> RecordKey {
    RecordKey::new(format!("#{number}"))
}

fn parse_record_key(key: &RecordKey) -> Option<u64> {
    key.as_str().strip_prefix('#')?.parse().ok()
}

fn objective_label(objective_id: &ObjectiveId) -> String {
    format!("{OBJECTIVE_LABEL_PREFIX}{objective_id}")
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
        let token = GitHubAppTokenProvider::new(
            &config.app_id,
            &config.private_key,
            config.installation_id,
            &config.api_base_url,
            Some(InstallationTokenScope::work_state(config.repository.name())),
            USER_AGENT,
        )?;
        Self::with_token_source(config, Arc::new(token))
    }

    pub(crate) fn with_token_source(
        config: &GitHubWorkStateConfig,
        token: Arc<dyn AccessTokenSource>,
    ) -> Result<Self, PortError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| PortError::new(format!("Failed to build GitHub client: {error}")))?;
        let app_id = config.app_id.trim().parse().map_err(|error| {
            PortError::invalid_input(format!(
                "Invalid GitHub App ID `{}`: {error}",
                config.app_id
            ))
        })?;
        Ok(Self {
            http,
            app_id,
            token,
            repository: config.repository.clone(),
            api_base_url: config.api_base_url.trim_end_matches('/').to_string(),
            web_base_url: config.web_base_url.trim_end_matches('/').to_string(),
            known_labels: Mutex::new(BTreeSet::new()),
        })
    }

    #[must_use]
    pub fn repository_name(&self) -> String {
        self.repository.to_string()
    }

    fn repo_url(&self) -> String {
        format!(
            "{}/repos/{}/{}",
            self.api_base_url,
            self.repository.owner(),
            self.repository.name()
        )
    }

    /// Resolves a record to its issue number, refusing references outside the management
    /// repository (defense in depth: request URLs are always built from config, but a foreign
    /// reference indicates a bug or tampering).
    fn guard(&self, record: &RecordRef) -> Result<u64, WorkStateError> {
        let expected_prefix = format!(
            "{}/{}/{}/issues/",
            self.web_base_url,
            self.repository.owner(),
            self.repository.name()
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

    async fn add_label(&self, number: u64, label: String) -> Result<(), WorkStateError> {
        self.ensure_labels(std::slice::from_ref(&label)).await?;
        let _: Value = self
            .request_json(
                reqwest::Method::POST,
                &format!("{}/issues/{number}/labels", self.repo_url()),
                Some(&json!({ "labels": [label] })),
                "add label",
            )
            .await?;
        Ok(())
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
            .map(|issue| issue.to_work_item(objective_id, self.app_id))
            .collect())
    }

    async fn list_unlinked_reili_work(&self) -> Result<Vec<RecordRef>, WorkStateError> {
        let labels = [ASSIGNEE_LABEL.to_string()];
        Ok(self
            .list_issues(&labels, "open", None)
            .await?
            .iter()
            .filter(|issue| !issue.has_objective_label())
            .map(ApiIssue::record)
            .collect())
    }

    async fn link_to_objective(
        &self,
        record: &RecordRef,
        objective_id: &ObjectiveId,
    ) -> Result<(), WorkStateError> {
        let number = self.guard(record)?;
        self.add_label(number, objective_label(objective_id)).await
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
                let by_reili = performed_by(comment.performed_via_github_app.as_ref(), self.app_id);
                comments.push(WorkItemComment {
                    author_trusted: by_reili
                        || (!user.is_bot()
                            && TRUSTED_ASSOCIATIONS.contains(&comment.author_association.as_str())),
                    by_reili,
                    author: user.login,
                    body,
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
                    "body": truncate_chars(&item.body, MAX_BODY_CHARS),
                    "labels": labels,
                })),
                "create work item",
            )
            .await?;
        Ok(issue.to_work_item(&item.objective_id, self.app_id))
    }

    async fn add_comment(&self, record: &RecordRef, body: &str) -> Result<(), WorkStateError> {
        let number = self.guard(record)?;
        let _: Value = self
            .request_json(
                reqwest::Method::POST,
                &format!("{}/issues/{number}/comments", self.repo_url()),
                Some(&json!({ "body": truncate_chars(body, MAX_BODY_CHARS) })),
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
            return self
                .add_label(number, NEEDS_DECISION_LABEL.to_string())
                .await;
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

    async fn close_work_item(&self, record: &RecordRef) -> Result<(), WorkStateError> {
        let number = self.guard(record)?;
        let _: Value = self
            .request_json(
                reqwest::Method::PATCH,
                &format!("{}/issues/{number}", self.repo_url()),
                Some(&json!({ "state": "closed", "state_reason": "completed" })),
                "close work item",
            )
            .await?;
        Ok(())
    }
}
