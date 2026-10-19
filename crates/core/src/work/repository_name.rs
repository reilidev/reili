use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// A GitHub repository written as `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryName {
    owner: String,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("repository must be `owner/name`, got `{0}`")]
pub struct InvalidRepositoryName(String);

impl RepositoryName {
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl FromStr for RepositoryName {
    type Err = InvalidRepositoryName;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().split_once('/') {
            Some((owner, name)) if is_valid_part(owner) && is_valid_part(name) => Ok(Self {
                owner: owner.to_string(),
                name: name.to_string(),
            }),
            _ => Err(InvalidRepositoryName(value.to_string())),
        }
    }
}

impl fmt::Display for RepositoryName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

fn is_valid_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::RepositoryName;

    #[test]
    fn parses_owner_and_name() {
        let repository: RepositoryName = " acme/reili-work.v2 ".parse().expect("valid");

        assert_eq!(repository.owner(), "acme");
        assert_eq!(repository.name(), "reili-work.v2");
        assert_eq!(repository.to_string(), "acme/reili-work.v2");
    }

    #[test]
    fn rejects_malformed_names() {
        for value in [
            "",
            "acme",
            "acme/",
            "/reili",
            "acme/reili/extra",
            "acme/reili work",
            "acme/réili",
        ] {
            assert!(
                value.parse::<RepositoryName>().is_err(),
                "{value:?} must be rejected"
            );
        }
    }
}
