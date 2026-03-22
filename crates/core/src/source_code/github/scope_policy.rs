use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubScopePolicy {
    scope_org: String,
}

impl GithubScopePolicy {
    pub fn new(scope_org: String) -> Result<Self, PortError> {
        let scope_org = normalize_org_name(&scope_org);

        if scope_org.is_empty() {
            return Err(PortError::invalid_input("github scope_org is required"));
        }

        Ok(Self { scope_org })
    }

    pub fn scope_org(&self) -> &str {
        &self.scope_org
    }

    pub fn validate_query(&self, query: &str) -> Result<(), PortError> {
        let org_qualifiers = extract_org_qualifiers(query);

        if org_qualifiers.is_empty() {
            return Err(PortError::invalid_input(format!(
                "org qualifier is required. include org:{}",
                self.scope_org
            )));
        }

        if org_qualifiers.iter().any(|org| org != self.scope_org()) {
            return Err(PortError::invalid_input(format!(
                "org qualifier is out of scope. allowed org: {}",
                self.scope_org
            )));
        }

        Ok(())
    }

    pub fn validate_owner(&self, owner: &str) -> Result<(), PortError> {
        if owner.eq_ignore_ascii_case(&self.scope_org) {
            return Ok(());
        }

        Err(PortError::invalid_input(format!(
            "owner is out of scope. allowed owner: {}",
            self.scope_org
        )))
    }
}

fn normalize_org_name(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn extract_org_qualifiers(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .filter_map(read_org_qualifier)
        .collect()
}

fn read_org_qualifier(token: &str) -> Option<String> {
    let cleaned = trim_qualifier_token(token);
    let (key, value) = cleaned.split_once(':')?;

    if !key.eq_ignore_ascii_case("org") {
        return None;
    }

    let normalized = normalize_org_name(trim_qualifier_token(value));

    if normalized.is_empty() {
        return None;
    }

    Some(normalized)
}

fn trim_qualifier_token(value: &str) -> &str {
    value.trim().trim_matches(|ch: char| {
        matches!(
            ch,
            '(' | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '"'
                | '\''
                | ','
                | '.'
                | ';'
                | '!'
                | '?'
                | '。'
                | '、'
        )
    })
}

#[cfg(test)]
mod tests {
    use super::GithubScopePolicy;

    #[test]
    fn allows_scoped_query_case_insensitively() {
        let policy = GithubScopePolicy::new(" AcMe ".to_string()).expect("create scope policy");

        let result = policy.validate_query("is:open org:AcMe repo:acme/service");

        assert!(result.is_ok());
        assert_eq!(policy.scope_org(), "acme");
    }

    #[test]
    fn rejects_query_without_org_qualifier() {
        let policy = GithubScopePolicy::new("acme".to_string()).expect("create scope policy");

        let error = policy
            .validate_query("is:open repo:acme/service")
            .expect_err("missing org qualifier should fail");

        assert_eq!(error.message, "org qualifier is required. include org:acme");
    }

    #[test]
    fn rejects_query_when_any_org_qualifier_is_out_of_scope() {
        let policy = GithubScopePolicy::new("acme".to_string()).expect("create scope policy");

        let error = policy
            .validate_query("is:open org:acme org:other repo:acme/service")
            .expect_err("out of scope org qualifier should fail");

        assert_eq!(
            error.message,
            "org qualifier is out of scope. allowed org: acme"
        );
    }

    #[test]
    fn reads_org_qualifier_from_token_with_punctuation() {
        let policy = GithubScopePolicy::new("acme".to_string()).expect("create scope policy");

        let result = policy.validate_query("label:bug (org:acme、) repo:acme/service");

        assert!(result.is_ok());
    }

    #[test]
    fn allows_owner_with_case_difference() {
        let policy = GithubScopePolicy::new("acme".to_string()).expect("create scope policy");

        let result = policy.validate_owner("AcMe");

        assert!(result.is_ok());
    }

    #[test]
    fn rejects_empty_scope_org_in_constructor() {
        let error = GithubScopePolicy::new("   ".to_string())
            .expect_err("empty scope org should be rejected");

        assert!(error.is_invalid_input());
        assert_eq!(error.message, "github scope_org is required");
    }
}
