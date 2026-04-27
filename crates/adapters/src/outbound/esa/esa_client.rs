use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::secret::SecretString;
use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};

use super::{
    EsaPost, EsaPostSearchInput, EsaPostSearchOrder, EsaPostSearchPort, EsaPostSearchResult,
    EsaPostSearchSort, EsaUser,
};
use crate::json_utils::truncate_for_error;

const DEFAULT_BASE_URL: &str = "https://api.esa.io";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsaClientConfig {
    pub access_token: SecretString,
    pub team_name: String,
}

#[derive(Debug, Clone)]
pub struct EsaClient {
    client: reqwest::Client,
    access_token: SecretString,
    team_name: String,
    base_url: String,
}

#[derive(Debug, Serialize)]
struct SearchPostsQuery {
    q: String,
    page: u32,
    per_page: u32,
    sort: EsaPostSearchSort,
    order: EsaPostSearchOrder,
}

#[derive(Debug, Deserialize)]
struct SearchPostsResponseDto {
    #[serde(default)]
    posts: Vec<EsaPostDto>,
    prev_page: Option<u32>,
    next_page: Option<u32>,
    #[serde(default)]
    total_count: u32,
    #[serde(default)]
    page: u32,
    #[serde(default)]
    per_page: u32,
    #[serde(default)]
    max_per_page: u32,
}

#[derive(Debug, Deserialize)]
struct EsaPostDto {
    #[serde(default)]
    number: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    wip: bool,
    #[serde(default)]
    body_md: String,
    url: Option<String>,
    category: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    created_by: Option<EsaUserDto>,
    updated_by: Option<EsaUserDto>,
    comments_count: Option<u32>,
    watchers_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct EsaUserDto {
    name: Option<String>,
    screen_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EsaErrorResponseDto {
    error: Option<String>,
    message: Option<String>,
}

impl EsaClient {
    pub fn new(config: EsaClientConfig) -> Result<Self, PortError> {
        Self::new_with_base_url(config, DEFAULT_BASE_URL)
    }

    fn new_with_base_url(config: EsaClientConfig, base_url: &str) -> Result<Self, PortError> {
        let access_token = config.access_token;
        if access_token.expose().trim().is_empty() {
            return Err(PortError::invalid_input(
                "esa access token must not be empty",
            ));
        }

        let team_name = config.team_name;
        let base_url = base_url.to_string();
        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| PortError::new(format!("Failed to build esa HTTP client: {error}")))?;

        Ok(Self {
            client,
            access_token,
            team_name,
            base_url,
        })
    }
}

#[async_trait]
impl EsaPostSearchPort for EsaClient {
    async fn search_posts(
        &self,
        input: EsaPostSearchInput,
    ) -> Result<EsaPostSearchResult, PortError> {
        let query = SearchPostsQuery {
            q: input.q.trim().to_string(),
            page: input.page,
            per_page: input.per_page,
            sort: input.sort,
            order: input.order,
        };
        let url = format!("{}/v1/teams/{}/posts", self.base_url, self.team_name);
        let response = self
            .client
            .get(&url)
            .bearer_auth(self.access_token.expose())
            .query(&query)
            .send()
            .await
            .map_err(|error| {
                PortError::connection_failed(format!(
                    "esa API request failed: endpoint=search_posts error={error}"
                ))
            })?;

        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.bytes().await.map_err(|error| {
            PortError::invalid_response(format!(
                "Failed to read esa API response body: endpoint=search_posts error={error}"
            ))
        })?;

        if !status.is_success() {
            return Err(map_esa_error_response(status, &headers, &bytes));
        }

        let parsed: SearchPostsResponseDto = serde_json::from_slice(&bytes).map_err(|error| {
            PortError::invalid_response(format!(
                "Failed to parse esa search_posts response JSON: {error}"
            ))
        })?;

        Ok(parsed.into())
    }
}

impl From<SearchPostsResponseDto> for EsaPostSearchResult {
    fn from(value: SearchPostsResponseDto) -> Self {
        Self {
            posts: value.posts.into_iter().map(Into::into).collect(),
            prev_page: value.prev_page,
            next_page: value.next_page,
            total_count: value.total_count,
            page: value.page,
            per_page: value.per_page,
            max_per_page: value.max_per_page,
        }
    }
}

impl From<EsaPostDto> for EsaPost {
    fn from(value: EsaPostDto) -> Self {
        Self {
            number: value.number,
            name: value.name,
            wip: value.wip,
            body_md: value.body_md,
            url: trim_optional_string(value.url),
            category: trim_optional_string(value.category),
            tags: value.tags,
            created_at: trim_optional_string(value.created_at),
            updated_at: trim_optional_string(value.updated_at),
            created_by: value.created_by.map(Into::into),
            updated_by: value.updated_by.map(Into::into),
            comments_count: value.comments_count,
            watchers_count: value.watchers_count,
        }
    }
}

impl From<EsaUserDto> for EsaUser {
    fn from(value: EsaUserDto) -> Self {
        Self {
            name: trim_optional_string(value.name),
            screen_name: trim_optional_string(value.screen_name),
        }
    }
}

fn map_esa_error_response(status: StatusCode, headers: &HeaderMap, bytes: &[u8]) -> PortError {
    let api_error = serde_json::from_slice::<EsaErrorResponseDto>(bytes).ok();
    let error_code = api_error
        .as_ref()
        .and_then(|error| trim_optional_string(error.error.clone()))
        .unwrap_or_else(|| "unknown_error".to_string());
    let api_message = api_error
        .as_ref()
        .and_then(|error| trim_optional_string(error.message.clone()));
    let mut message = format!(
        "esa API request failed: endpoint=search_posts status={} error={error_code}",
        status.as_u16()
    );

    if let Some(api_message) = api_message {
        message.push_str(" message=");
        message.push_str(&truncate_for_error(&api_message));
    } else if !bytes.is_empty() {
        message.push_str(" body=");
        message.push_str(&truncate_for_error(String::from_utf8_lossy(bytes).as_ref()));
    }

    let rate_limit_context = format_rate_limit_context(headers);
    if !rate_limit_context.is_empty() {
        message.push(' ');
        message.push_str(&rate_limit_context);
    }

    PortError::http_status(status.as_u16(), message)
}

fn format_rate_limit_context(headers: &HeaderMap) -> String {
    let pairs = [
        ("retry-after", "retryAfter"),
        ("x-ratelimit-limit", "rateLimit"),
        ("x-ratelimit-remaining", "rateLimitRemaining"),
        ("x-ratelimit-reset", "rateLimitReset"),
    ]
    .into_iter()
    .filter_map(|(header_name, output_name)| {
        headers
            .get(header_name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("{output_name}={value}"))
    })
    .collect::<Vec<_>>();

    pairs.join(" ")
}

fn trim_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use reili_core::error::PortErrorKind;
    use reili_core::secret::SecretString;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{
        EsaClient, EsaClientConfig, EsaPostSearchInput, EsaPostSearchOrder, EsaPostSearchPort,
        EsaPostSearchSort,
    };

    #[tokio::test]
    async fn gets_posts_with_bearer_token_and_query_params() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/teams/docs/posts"))
            .and(header("Authorization", "Bearer esa-token"))
            .and(query_param("q", "in:runbooks error"))
            .and(query_param("page", "2"))
            .and(query_param("per_page", "3"))
            .and(query_param("sort", "best_match"))
            .and(query_param("order", "desc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "posts": [{
                    "number": 102,
                    "name": "Runbook",
                    "wip": false,
                    "body_md": "# Runbook\nCheck dashboards.",
                    "url": "https://docs.esa.io/posts/102",
                    "category": "SRE",
                    "tags": ["alert"],
                    "created_at": "2026-01-01T00:00:00+09:00",
                    "updated_at": "2026-01-02T00:00:00+09:00",
                    "revision_number": 7,
                    "created_by": {
                        "myself": false,
                        "name": "Jane",
                        "screen_name": "jane",
                        "icon": "https://img.example/icon.png"
                    },
                    "updated_by": {
                        "screen_name": "john"
                    },
                    "kind": "stock",
                    "comments_count": 1,
                    "tasks_count": 2,
                    "done_tasks_count": 1,
                    "stargazers_count": 4,
                    "watchers_count": 5,
                    "star": false,
                    "watch": true
                }],
                "prev_page": 1,
                "next_page": 3,
                "total_count": 9,
                "page": 2,
                "per_page": 3,
                "max_per_page": 100
            })))
            .mount(&server)
            .await;

        let client = create_client(&server.uri());
        let result = client
            .search_posts(EsaPostSearchInput {
                q: " in:runbooks error ".to_string(),
                page: 2,
                per_page: 3,
                sort: EsaPostSearchSort::BestMatch,
                order: EsaPostSearchOrder::Desc,
            })
            .await
            .expect("search posts");

        assert_eq!(result.posts.len(), 1);
        assert_eq!(result.posts[0].number, 102);
        assert_eq!(result.posts[0].body_md, "# Runbook\nCheck dashboards.");
        assert_eq!(result.posts[0].watchers_count, Some(5));
        assert_eq!(
            result.posts[0]
                .created_by
                .as_ref()
                .and_then(|user| user.screen_name.as_deref()),
            Some("jane")
        );
        assert_eq!(result.prev_page, Some(1));
        assert_eq!(result.next_page, Some(3));
        assert_eq!(result.total_count, 9);
    }

    #[tokio::test]
    async fn maps_error_response_with_rate_limit_headers() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/teams/docs/posts"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "30")
                    .insert_header("x-ratelimit-remaining", "0")
                    .set_body_json(serde_json::json!({
                        "error": "rate_limit_exceeded",
                        "message": "Retry after 30 seconds."
                    })),
            )
            .mount(&server)
            .await;

        let client = create_client(&server.uri());
        let error = client
            .search_posts(EsaPostSearchInput {
                q: "runbook".to_string(),
                page: 1,
                per_page: 5,
                sort: EsaPostSearchSort::Updated,
                order: EsaPostSearchOrder::Desc,
            })
            .await
            .expect_err("rate limit should fail");

        assert_eq!(error.kind, PortErrorKind::HttpStatus { status_code: 429 });
        assert!(error.message.contains("rate_limit_exceeded"));
        assert!(error.message.contains("retryAfter=30"));
        assert!(error.message.contains("rateLimitRemaining=0"));
    }

    #[test]
    fn rejects_empty_access_token() {
        let empty_token = EsaClient::new(EsaClientConfig {
            access_token: SecretString::from(" "),
            team_name: "docs".to_string(),
        })
        .expect_err("empty token");
        assert!(empty_token.is_invalid_input());
    }

    fn create_client(base_url: &str) -> EsaClient {
        EsaClient::new_with_base_url(
            EsaClientConfig {
                access_token: SecretString::from("esa-token"),
                team_name: "docs".to_string(),
            },
            base_url,
        )
        .expect("create client")
    }
}
