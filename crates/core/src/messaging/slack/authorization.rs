use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackAuthorizationPolicy {
    channel_name_patterns: Option<Vec<String>>,
    actor_user_ids: Option<Vec<String>>,
    actor_user_group_ids: Option<Vec<String>>,
    actor_allow_bot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackAuthorizationContext<'a> {
    pub channel_name: Option<&'a str>,
    pub user_id: &'a str,
    pub actor_is_bot: bool,
    pub matching_user_group_ids: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackAuthorizationDecision {
    Allow,
    Deny {
        reason: SlackAuthorizationDenyReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackAuthorizationDenyReason {
    PrivateChannelNotAllowed,
    ChannelNameUnavailable,
    ChannelNotAllowed,
    ActorNotAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackChannelInfo {
    pub name: String,
    pub is_private: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostSlackEphemeralMessageInput {
    pub channel: String,
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    pub text: String,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackChannelLookupPort: Send + Sync {
    async fn lookup_channel_info(&self, channel_id: &str) -> Result<SlackChannelInfo, PortError>;
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackUserGroupMembershipPort: Send + Sync {
    async fn list_user_group_members(&self, user_group_id: &str) -> Result<Vec<String>, PortError>;
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackEphemeralMessagePort: Send + Sync {
    async fn post_ephemeral_message(
        &self,
        input: PostSlackEphemeralMessageInput,
    ) -> Result<(), PortError>;
}

impl SlackAuthorizationPolicy {
    pub fn new(
        channel_name_patterns: Option<Vec<String>>,
        actor_user_ids: Option<Vec<String>>,
        actor_user_group_ids: Option<Vec<String>>,
        actor_allow_bot: bool,
    ) -> Self {
        Self {
            channel_name_patterns,
            actor_user_ids,
            actor_user_group_ids,
            actor_allow_bot,
        }
    }

    pub fn has_channel_name_condition(&self) -> bool {
        self.channel_name_patterns.is_some()
    }

    pub fn requires_channel_name_lookup(&self) -> bool {
        self.channel_name_patterns
            .as_ref()
            .is_some_and(|patterns| !patterns.is_empty())
    }

    pub fn has_actor_condition(&self) -> bool {
        self.actor_user_ids.is_some() || self.actor_user_group_ids.is_some() || self.actor_allow_bot
    }

    pub fn has_user_group_condition(&self) -> bool {
        self.actor_user_group_ids
            .as_ref()
            .is_some_and(|ids| !ids.is_empty())
    }

    pub fn is_direct_user_allowed(&self, user_id: &str) -> bool {
        self.actor_user_ids
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|id| id == user_id))
    }

    pub fn is_bot_allowed(&self, actor_is_bot: bool) -> bool {
        self.actor_allow_bot && actor_is_bot
    }

    pub fn user_group_ids(&self) -> &[String] {
        self.actor_user_group_ids.as_deref().unwrap_or(&[])
    }

    pub fn decide(&self, context: SlackAuthorizationContext<'_>) -> SlackAuthorizationDecision {
        if let Some(channel_name_patterns) = &self.channel_name_patterns {
            if channel_name_patterns.is_empty() {
                return SlackAuthorizationDecision::Deny {
                    reason: SlackAuthorizationDenyReason::ChannelNotAllowed,
                };
            }

            let Some(channel_name) = context.channel_name.filter(|value| !value.is_empty()) else {
                return SlackAuthorizationDecision::Deny {
                    reason: SlackAuthorizationDenyReason::ChannelNameUnavailable,
                };
            };

            if !channel_name_patterns
                .iter()
                .any(|pattern| wildcard_match(pattern, channel_name))
            {
                return SlackAuthorizationDecision::Deny {
                    reason: SlackAuthorizationDenyReason::ChannelNotAllowed,
                };
            }
        }

        if self.has_actor_condition() {
            let direct_user_allowed = self
                .actor_user_ids
                .as_ref()
                .is_some_and(|ids| ids.iter().any(|id| id == context.user_id));
            let user_group_allowed = self.actor_user_group_ids.as_ref().is_some_and(|ids| {
                context
                    .matching_user_group_ids
                    .iter()
                    .any(|value| ids.contains(value))
            });
            let bot_allowed = self.is_bot_allowed(context.actor_is_bot);

            if !direct_user_allowed && !user_group_allowed && !bot_allowed {
                return SlackAuthorizationDecision::Deny {
                    reason: SlackAuthorizationDenyReason::ActorNotAllowed,
                };
            }
        }

        SlackAuthorizationDecision::Allow
    }
}

impl SlackAuthorizationDenyReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrivateChannelNotAllowed => "private_channel_not_allowed",
            Self::ChannelNameUnavailable => "channel_name_unavailable",
            Self::ChannelNotAllowed => "channel_not_allowed",
            Self::ActorNotAllowed => "actor_not_allowed",
        }
    }
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if !pattern.contains('*') {
        return pattern == value;
    }

    let starts_with_wildcard = pattern.starts_with('*');
    let ends_with_wildcard = pattern.ends_with('*');
    let parts: Vec<&str> = pattern.split('*').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return true;
    }

    let mut remainder = value;
    let mut next_part_index = 0;

    if !starts_with_wildcard {
        let first_part = parts[0];
        if !remainder.starts_with(first_part) {
            return false;
        }
        remainder = &remainder[first_part.len()..];
        next_part_index = 1;
    }

    for part in &parts[next_part_index..] {
        let Some(position) = remainder.find(part) else {
            return false;
        };
        remainder = &remainder[position + part.len()..];
    }

    ends_with_wildcard || value.ends_with(parts[parts.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::{
        SlackAuthorizationContext, SlackAuthorizationDecision, SlackAuthorizationDenyReason,
        SlackAuthorizationPolicy,
    };

    fn create_policy(
        channel_name_patterns: Option<Vec<&str>>,
        actor_user_ids: Option<Vec<&str>>,
        actor_user_group_ids: Option<Vec<&str>>,
    ) -> SlackAuthorizationPolicy {
        create_policy_with_allow_bot(
            channel_name_patterns,
            actor_user_ids,
            actor_user_group_ids,
            false,
        )
    }

    fn create_policy_with_allow_bot(
        channel_name_patterns: Option<Vec<&str>>,
        actor_user_ids: Option<Vec<&str>>,
        actor_user_group_ids: Option<Vec<&str>>,
        actor_allow_bot: bool,
    ) -> SlackAuthorizationPolicy {
        SlackAuthorizationPolicy::new(
            channel_name_patterns.map(to_strings),
            actor_user_ids.map(to_strings),
            actor_user_group_ids.map(to_strings),
            actor_allow_bot,
        )
    }

    fn to_strings(values: Vec<&str>) -> Vec<String> {
        values.into_iter().map(ToString::to_string).collect()
    }

    #[test]
    fn allows_exact_channel_name_pattern_match() {
        let policy = create_policy(Some(vec!["alerts-prod"]), None, None);

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("alerts-prod"),
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Allow
        );
    }

    #[test]
    fn matches_channel_name_patterns_without_normalization() {
        let policy = create_policy(Some(vec!["Alerts-*", " deploy-prod "]), None, None);

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("alerts-prod"),
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Deny {
                reason: SlackAuthorizationDenyReason::ChannelNotAllowed,
            }
        );
        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("deploy-prod"),
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Deny {
                reason: SlackAuthorizationDenyReason::ChannelNotAllowed,
            }
        );
    }

    #[test]
    fn allows_leading_wildcard_channel_name_pattern_match() {
        let policy = create_policy(Some(vec!["*-prod"]), None, None);

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("alerts-prod"),
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Allow
        );
    }

    #[test]
    fn allows_trailing_wildcard_channel_name_pattern_match() {
        let policy = create_policy(Some(vec!["alerts-*"]), None, None);

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("alerts-prod"),
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Allow
        );
    }

    #[test]
    fn allows_middle_wildcard_channel_name_pattern_match() {
        let policy = create_policy(Some(vec!["test-*-x"]), None, None);

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("test-prod-x"),
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Allow
        );
        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("test-障害対応-x"),
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Allow
        );
        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("test-prod-x-extra"),
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Deny {
                reason: SlackAuthorizationDenyReason::ChannelNotAllowed,
            }
        );
    }

    #[test]
    fn denies_non_matching_channel_name_pattern() {
        let policy = create_policy(Some(vec!["alerts-*"]), None, None);

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("deploy-prod"),
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Deny {
                reason: SlackAuthorizationDenyReason::ChannelNotAllowed,
            }
        );
    }

    #[test]
    fn denies_when_channel_name_pattern_list_is_explicitly_empty() {
        let policy = create_policy(Some(Vec::new()), None, None);

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: None,
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Deny {
                reason: SlackAuthorizationDenyReason::ChannelNotAllowed,
            }
        );
    }

    #[test]
    fn allows_direct_user_id_match() {
        let policy = create_policy(None, Some(vec!["U001"]), None);

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: None,
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Allow
        );
    }

    #[test]
    fn matches_actor_ids_without_normalization() {
        let policy = create_policy(None, Some(vec!["u001", " U001 "]), Some(vec!["s001"]));

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: None,
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &["S001".to_string()],
            }),
            SlackAuthorizationDecision::Deny {
                reason: SlackAuthorizationDenyReason::ActorNotAllowed,
            }
        );
    }

    #[test]
    fn denies_when_direct_user_id_list_is_explicitly_empty() {
        let policy = create_policy(None, Some(Vec::new()), None);

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: None,
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Deny {
                reason: SlackAuthorizationDenyReason::ActorNotAllowed,
            }
        );
    }

    #[test]
    fn allows_user_group_match() {
        let policy = create_policy(None, None, Some(vec!["S001"]));

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: None,
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &["S001".to_string()],
            }),
            SlackAuthorizationDecision::Allow
        );
    }

    #[test]
    fn denies_when_user_group_id_list_is_explicitly_empty() {
        let policy = create_policy(None, None, Some(Vec::new()));

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: None,
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Deny {
                reason: SlackAuthorizationDenyReason::ActorNotAllowed,
            }
        );
    }

    #[test]
    fn requires_channel_and_actor_conditions_when_both_are_configured() {
        let policy = create_policy(Some(vec!["alerts-*"]), Some(vec!["U001"]), None);

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("alerts-prod"),
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Allow
        );
        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("deploy-prod"),
                user_id: "U001",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Deny {
                reason: SlackAuthorizationDenyReason::ChannelNotAllowed,
            }
        );
        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("alerts-prod"),
                user_id: "U002",
                actor_is_bot: false,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Deny {
                reason: SlackAuthorizationDenyReason::ActorNotAllowed,
            }
        );
    }

    #[test]
    fn allows_bot_actor_when_bot_actors_are_allowed() {
        let policy = create_policy_with_allow_bot(Some(vec!["alerts-*"]), None, None, true);

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("alerts-prod"),
                user_id: "U-BOT-ACTOR",
                actor_is_bot: true,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Allow
        );
    }

    #[test]
    fn denies_bot_actor_when_bot_actors_are_not_allowed() {
        let policy = create_policy_with_allow_bot(
            Some(vec!["alerts-*"]),
            Some(vec!["U-HUMAN"]),
            None,
            false,
        );

        assert_eq!(
            policy.decide(SlackAuthorizationContext {
                channel_name: Some("alerts-prod"),
                user_id: "U-BOT-ACTOR",
                actor_is_bot: true,
                matching_user_group_ids: &[],
            }),
            SlackAuthorizationDecision::Deny {
                reason: SlackAuthorizationDenyReason::ActorNotAllowed,
            }
        );
    }
}
