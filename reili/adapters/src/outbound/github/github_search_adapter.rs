use async_trait::async_trait;
use octocrab::models::{AppId, InstallationId};
use octocrab::{Octocrab, models};
use serde_json::Value;
use reili_shared::errors::PortError;
use reili_shared::ports::outbound::github_search::{
    GithubRepositoryDirectoryContent, GithubRepositoryDirectoryEntry, GithubRepositoryFileContent,
    GithubRepositoryFileEncoding,
};
use reili_shared::ports::outbound::{
    GithubCodeSearchPort, GithubCodeSearchResultItem, GithubIssueSearchResultItem,
    GithubPullRequestDiff, GithubPullRequestParams, GithubPullRequestPort,
    GithubPullRequestSummary, GithubRepoSearchResultItem, GithubRepositoryContent,
    GithubRepositoryContentParams, GithubRepositoryContentPort, GithubSearchParams,
};

const MAX_RESULTS_PER_PAGE: u8 = 30;
const MAX_CONTENT_BYTES: usize = 200_000;
const MAX_CONTENT_CHARS: usize = 50_000;
const MAX_DIFF_CHARS: usize = 120_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubSearchAdapterConfig {
    pub app_id: String,
    pub private_key: String,
    pub installation_id: u32,
    pub scope_org: String,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GitHubSearchAdapter {
    config: GitHubSearchAdapterConfig,
    client: Octocrab,
}

impl GitHubSearchAdapter {
    pub fn new(config: GitHubSearchAdapterConfig) -> Result<Self, PortError> {
        let app_id = parse_app_id(&config.app_id)?;
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(config.private_key.as_bytes()).map_err(
            |error| PortError::new(format!("Failed to parse GitHub App private key: {error}")),
        )?;

        let mut builder = Octocrab::builder().app(AppId(app_id), key);
        if let Some(base_url) = config.base_url.as_ref() {
            builder = builder.base_uri(base_url).map_err(map_octocrab_error)?;
        }

        let app_client = builder.build().map_err(map_octocrab_error)?;
        let installation_client =
            app_client.installation(InstallationId(u64::from(config.installation_id)));

        Ok(Self {
            config,
            client: installation_client,
        })
    }

    fn require_org_scoped_query(&self, query: &str) -> Result<(), PortError> {
        ensure_org_scoped_query(query, &self.config.scope_org)
    }

    fn require_owner_in_scope(&self, owner: &str) -> Result<(), PortError> {
        ensure_owner_in_scope(owner, &self.config.scope_org)
    }
}

#[async_trait]
impl GithubCodeSearchPort for GitHubSearchAdapter {
    async fn search_code(
        &self,
        params: GithubSearchParams,
    ) -> Result<Vec<GithubCodeSearchResultItem>, PortError> {
        self.require_org_scoped_query(&params.query)?;

        let page = self
            .client
            .search()
            .code(&params.query)
            .per_page(to_per_page(params.limit))
            .send()
            .await
            .map_err(map_octocrab_error)?;

        Ok(page
            .items
            .into_iter()
            .map(|item| GithubCodeSearchResultItem {
                name: item.name,
                path: item.path,
                repository_full_name: item
                    .repository
                    .full_name
                    .unwrap_or_else(|| item.repository.name.clone()),
                html_url: item.html_url.to_string(),
            })
            .collect())
    }

    async fn search_repos(
        &self,
        params: GithubSearchParams,
    ) -> Result<Vec<GithubRepoSearchResultItem>, PortError> {
        self.require_org_scoped_query(&params.query)?;

        let page = self
            .client
            .search()
            .repositories(&params.query)
            .per_page(to_per_page(params.limit))
            .send()
            .await
            .map_err(map_octocrab_error)?;

        Ok(page
            .items
            .into_iter()
            .map(|item| GithubRepoSearchResultItem {
                full_name: item.full_name.unwrap_or(item.name),
                description: item.description,
                html_url: item
                    .html_url
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                default_branch: item.default_branch.unwrap_or_default(),
                language: map_repo_language(item.language),
                updated_at: item
                    .updated_at
                    .map(|value| value.to_rfc3339())
                    .unwrap_or_else(|| "unknown".to_string()),
            })
            .collect())
    }

    async fn search_issues_and_pull_requests(
        &self,
        params: GithubSearchParams,
    ) -> Result<Vec<GithubIssueSearchResultItem>, PortError> {
        self.require_org_scoped_query(&params.query)?;

        let page = self
            .client
            .search()
            .issues_and_pull_requests(&params.query)
            .per_page(to_per_page(params.limit))
            .send()
            .await
            .map_err(map_octocrab_error)?;

        Ok(page
            .items
            .into_iter()
            .map(|item| GithubIssueSearchResultItem {
                number: item.number,
                title: item.title,
                state: match item.state {
                    models::IssueState::Open => "open".to_string(),
                    models::IssueState::Closed => "closed".to_string(),
                    _ => "unknown".to_string(),
                },
                html_url: item.html_url.to_string(),
                repository_url: item.repository_url.to_string(),
                user_login: Some(item.user.login),
                created_at: item.created_at.to_rfc3339(),
                updated_at: item.updated_at.to_rfc3339(),
                pull_request: item.pull_request.is_some(),
            })
            .collect())
    }
}

#[async_trait]
impl GithubRepositoryContentPort for GitHubSearchAdapter {
    async fn get_repository_content(
        &self,
        params: GithubRepositoryContentParams,
    ) -> Result<GithubRepositoryContent, PortError> {
        self.require_owner_in_scope(&params.owner)?;

        let repo_handler = self.client.repos(params.owner.clone(), params.repo);
        let mut request = repo_handler.get_content().path(params.path.clone());

        if let Some(reference) = params.r#ref {
            request = request.r#ref(reference);
        }

        let mut content_items = request.send().await.map_err(map_octocrab_error)?;
        let entries = content_items.take_items();

        if entries.len() == 1
            && let Some(file) = entries.first()
            && file.r#type == "file"
        {
            let decoded = decode_base64(file.content.as_deref().unwrap_or_default())?;
            let original_bytes = decoded.len();
            let capped_bytes = if original_bytes > MAX_CONTENT_BYTES {
                &decoded[..MAX_CONTENT_BYTES]
            } else {
                &decoded
            };
            let utf8_content = String::from_utf8_lossy(capped_bytes).to_string();
            let (content, chars_truncated) =
                truncate_to_char_limit(&utf8_content, MAX_CONTENT_CHARS);
            let returned_chars = u64::try_from(count_chars(&content)).unwrap_or(u64::MAX);

            return Ok(GithubRepositoryContent::File(GithubRepositoryFileContent {
                content,
                encoding: GithubRepositoryFileEncoding::Utf8,
                html_url: file
                    .html_url
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                original_bytes: u64::try_from(original_bytes).unwrap_or(u64::MAX),
                returned_chars,
                truncated: original_bytes > MAX_CONTENT_BYTES || chars_truncated,
            }));
        }

        Ok(GithubRepositoryContent::Directory(
            GithubRepositoryDirectoryContent {
                html_url: String::new(),
                entries: entries
                    .into_iter()
                    .map(|entry| GithubRepositoryDirectoryEntry {
                        name: entry.name,
                        path: entry.path,
                        entry_type: entry.r#type,
                    })
                    .collect(),
            },
        ))
    }
}

#[async_trait]
impl GithubPullRequestPort for GitHubSearchAdapter {
    async fn get_pull_request(
        &self,
        params: GithubPullRequestParams,
    ) -> Result<GithubPullRequestSummary, PortError> {
        self.require_owner_in_scope(&params.owner)?;

        let pr = self
            .client
            .pulls(params.owner.clone(), params.repo.clone())
            .get(params.pull_number)
            .await
            .map_err(map_octocrab_error)?;

        Ok(GithubPullRequestSummary {
            number: pr.number,
            state: match pr.state {
                Some(models::IssueState::Open) => "open".to_string(),
                Some(models::IssueState::Closed) => "closed".to_string(),
                Some(_) | None => "unknown".to_string(),
            },
            title: pr.title.unwrap_or_default(),
            body: pr.body,
            user_login: pr.user.map(|user| user.login),
            created_at: pr.created_at.map(|value| value.to_rfc3339()),
            updated_at: pr.updated_at.map(|value| value.to_rfc3339()),
            merged_at: pr.merged_at.map(|value| value.to_rfc3339()),
            additions: pr.additions,
            deletions: pr.deletions,
            changed_files: pr.changed_files,
            commits: pr.commits,
            html_url: pr
                .html_url
                .map(|value| value.to_string())
                .unwrap_or_else(|| {
                    format!(
                        "https://github.com/{}/{}/pull/{}",
                        params.owner, params.repo, params.pull_number
                    )
                }),
            base_ref: Some(pr.base.ref_field),
            head_ref: Some(pr.head.ref_field),
        })
    }

    async fn get_pull_request_diff(
        &self,
        params: GithubPullRequestParams,
    ) -> Result<GithubPullRequestDiff, PortError> {
        self.require_owner_in_scope(&params.owner)?;

        let raw_diff = self
            .client
            .pulls(params.owner.clone(), params.repo.clone())
            .get_diff(params.pull_number)
            .await
            .map_err(map_octocrab_error)?;

        let original_chars = count_chars(&raw_diff);
        let (truncated_diff, is_truncated) = truncate_to_char_limit(&raw_diff, MAX_DIFF_CHARS);
        let diff = if is_truncated {
            format!("{truncated_diff}\n\n... [truncated]")
        } else {
            truncated_diff
        };

        Ok(GithubPullRequestDiff {
            diff: diff.clone(),
            html_url: format!(
                "https://github.com/{}/{}/pull/{}",
                params.owner, params.repo, params.pull_number
            ),
            original_chars: u64::try_from(original_chars).unwrap_or(u64::MAX),
            returned_chars: u64::try_from(count_chars(&diff)).unwrap_or(u64::MAX),
            truncated: is_truncated,
        })
    }
}

fn parse_app_id(app_id: &str) -> Result<u64, PortError> {
    app_id
        .parse::<u64>()
        .map_err(|error| PortError::new(format!("Invalid GitHub App ID `{app_id}`: {error}")))
}

fn map_repo_language(language: Option<Value>) -> Option<String> {
    match language {
        Some(Value::String(value)) => Some(value),
        Some(Value::Number(value)) => Some(value.to_string()),
        Some(Value::Bool(value)) => Some(value.to_string()),
        Some(Value::Array(value)) => Some(Value::Array(value).to_string()),
        Some(Value::Object(value)) => Some(Value::Object(value).to_string()),
        Some(Value::Null) => None,
        None => None,
    }
}

fn to_per_page(limit: u32) -> u8 {
    if limit == 0 {
        return 1;
    }

    let clamped = limit.min(u32::from(MAX_RESULTS_PER_PAGE));
    u8::try_from(clamped).unwrap_or(MAX_RESULTS_PER_PAGE)
}

fn map_octocrab_error(error: octocrab::Error) -> PortError {
    PortError::new(format!("GitHub API request failed: {error}"))
}

fn normalize_org_name(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn ensure_org_scoped_query(query: &str, scope_org: &str) -> Result<(), PortError> {
    let target_org = normalize_org_name(scope_org);
    let org_qualifiers = extract_org_qualifiers(query);

    if org_qualifiers.is_empty() {
        return Err(PortError::new(format!(
            "org qualifier is required. include org:{scope_org}"
        )));
    }

    if org_qualifiers.iter().any(|org| org != &target_org) {
        return Err(PortError::new(format!(
            "org qualifier is out of scope. allowed org: {scope_org}"
        )));
    }

    Ok(())
}

fn ensure_owner_in_scope(owner: &str, scope_org: &str) -> Result<(), PortError> {
    if !owner.eq_ignore_ascii_case(scope_org) {
        return Err(PortError::new(format!(
            "owner is out of scope. allowed owner: {scope_org}"
        )));
    }

    Ok(())
}

fn extract_org_qualifiers(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .filter_map(read_org_qualifier)
        .collect()
}

fn read_org_qualifier(token: &str) -> Option<String> {
    let cleaned = token.trim().trim_matches(|ch: char| {
        matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | ',')
    });

    let (key, value) = cleaned.split_once(':')?;

    if !key.eq_ignore_ascii_case("org") {
        return None;
    }

    let normalized = value
        .trim()
        .trim_matches(|ch: char| matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | ','))
        .to_ascii_lowercase();

    if normalized.is_empty() {
        return None;
    }

    Some(normalized)
}

fn decode_base64(value: &str) -> Result<Vec<u8>, PortError> {
    use base64::Engine;

    let sanitized: Vec<u8> = value
        .as_bytes()
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();

    base64::prelude::BASE64_STANDARD
        .decode(sanitized)
        .map_err(|error| PortError::new(format!("Failed to decode repository content: {error}")))
}

fn truncate_to_char_limit(value: &str, max_chars: usize) -> (String, bool) {
    match value.char_indices().nth(max_chars) {
        Some((byte_index, _)) => (value[..byte_index].to_string(), true),
        None => (value.to_string(), false),
    }
}

fn count_chars(value: &str) -> usize {
    value.chars().count()
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_CONTENT_BYTES, MAX_CONTENT_CHARS, MAX_DIFF_CHARS, count_chars, decode_base64,
        ensure_org_scoped_query, ensure_owner_in_scope, extract_org_qualifiers, read_org_qualifier,
        truncate_to_char_limit,
    };

    #[test]
    fn extracts_org_qualifiers_case_insensitively() {
        let qualifiers = extract_org_qualifiers("is:open org:AcMe repo:acme/service");

        assert_eq!(qualifiers, vec!["acme"]);
    }

    #[test]
    fn reads_org_qualifier_with_trailing_punctuation() {
        assert_eq!(read_org_qualifier("(org:acme,)"), Some("acme".to_string()));
    }

    #[test]
    fn truncates_by_char_limit_without_splitting_utf8() {
        let input = "aあb";
        let (truncated, is_truncated) = truncate_to_char_limit(input, 2);

        assert_eq!(truncated, "aあ");
        assert!(is_truncated);
        assert_eq!(count_chars(&truncated), 2);
    }

    #[test]
    fn decodes_base64_with_whitespace() {
        let decoded = decode_base64("aGVs\nbG8=").expect("decode base64");

        assert_eq!(decoded, b"hello");
    }

    #[test]
    fn adapter_rejects_missing_org_qualifier() {
        let error = ensure_org_scoped_query("is:open repo:acme/service", "acme")
            .expect_err("missing org qualifier should fail");

        assert_eq!(error.message, "org qualifier is required. include org:acme");
    }

    #[test]
    fn adapter_rejects_out_of_scope_owner() {
        let error =
            ensure_owner_in_scope("other-org", "acme").expect_err("owner out of scope should fail");

        assert_eq!(error.message, "owner is out of scope. allowed owner: acme");
    }

    #[test]
    fn constants_match_phase_limits() {
        assert_eq!(MAX_CONTENT_BYTES, 200_000);
        assert_eq!(MAX_CONTENT_CHARS, 50_000);
        assert_eq!(MAX_DIFF_CHARS, 120_000);
    }
}
