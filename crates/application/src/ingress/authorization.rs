use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    PostSlackEphemeralMessageInput, SlackAuthorizationContext, SlackAuthorizationDecision,
    SlackAuthorizationDenyReason, SlackAuthorizationPolicy, SlackChannelInfo,
    SlackChannelLookupPort, SlackEphemeralMessagePort, SlackMessage, SlackMessageHandlerPort,
    SlackTriggerType, SlackUserGroupMembershipPort,
};

use crate::{TaskLogger, string_log_meta};

const DENY_EPHEMERAL_MESSAGE: &str =
    "I cannot respond to this mention because this channel or Slack user is not authorized to use.";
const CHANNEL_LOOKUP_FAILED_REASON: &str = "channel_lookup_failed";
const USER_GROUP_LOOKUP_FAILED_REASON: &str = "user_group_membership_lookup_failed";

pub struct SlackMentionAuthorizationService {
    authorization_policy: SlackAuthorizationPolicy,
    slack_channel_lookup_port: Arc<dyn SlackChannelLookupPort>,
    slack_user_group_membership_port: Arc<dyn SlackUserGroupMembershipPort>,
    slack_ephemeral_message_port: Arc<dyn SlackEphemeralMessagePort>,
    logger: Arc<dyn TaskLogger>,
}

pub struct SlackMentionAuthorizationGate {
    authorization_service: SlackMentionAuthorizationService,
    next_handler: Arc<dyn SlackMessageHandlerPort>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackMentionAuthorizationOutcome {
    Allowed,
    Denied,
}

struct AuthorizationResolution {
    decision: SlackAuthorizationDecision,
    channel_name: Option<String>,
}

struct AuthorizationFailure {
    reason: String,
    channel_name: Option<String>,
    error: PortError,
}

impl SlackMentionAuthorizationService {
    pub fn new(
        authorization_policy: SlackAuthorizationPolicy,
        slack_channel_lookup_port: Arc<dyn SlackChannelLookupPort>,
        slack_user_group_membership_port: Arc<dyn SlackUserGroupMembershipPort>,
        slack_ephemeral_message_port: Arc<dyn SlackEphemeralMessagePort>,
        logger: Arc<dyn TaskLogger>,
    ) -> Self {
        Self {
            authorization_policy,
            slack_channel_lookup_port,
            slack_user_group_membership_port,
            slack_ephemeral_message_port,
            logger,
        }
    }

    pub async fn authorize(
        &self,
        message: &SlackMessage,
    ) -> Result<SlackMentionAuthorizationOutcome, PortError> {
        let channel_info = match self.lookup_channel_info(message).await {
            Ok(channel_info) => channel_info,
            Err(failure) => {
                self.log_authorization_failure(message, &failure);
                self.deny_mention(
                    message,
                    failure.channel_name.as_deref(),
                    failure.reason.as_str(),
                )
                .await?;
                return Ok(SlackMentionAuthorizationOutcome::Denied);
            }
        };

        if channel_info.is_private {
            self.deny_mention(
                message,
                Some(&channel_info.name),
                SlackAuthorizationDenyReason::PrivateChannelNotAllowed.as_str(),
            )
            .await?;
            return Ok(SlackMentionAuthorizationOutcome::Denied);
        }

        match self
            .resolve_authorization(&self.authorization_policy, message, &channel_info)
            .await
        {
            Ok(AuthorizationResolution {
                decision: SlackAuthorizationDecision::Allow,
                ..
            }) => Ok(SlackMentionAuthorizationOutcome::Allowed),
            Ok(AuthorizationResolution {
                decision: SlackAuthorizationDecision::Deny { reason },
                channel_name,
            }) => {
                self.deny_mention(message, channel_name.as_deref(), reason.as_str())
                    .await?;
                Ok(SlackMentionAuthorizationOutcome::Denied)
            }
            Err(failure) => {
                self.log_authorization_failure(message, &failure);
                self.deny_mention(
                    message,
                    failure.channel_name.as_deref(),
                    failure.reason.as_str(),
                )
                .await?;
                Ok(SlackMentionAuthorizationOutcome::Denied)
            }
        }
    }

    async fn resolve_authorization(
        &self,
        policy: &SlackAuthorizationPolicy,
        message: &SlackMessage,
        channel_info: &SlackChannelInfo,
    ) -> Result<AuthorizationResolution, AuthorizationFailure> {
        let channel_name = if policy.has_channel_name_condition() {
            Some(channel_info.name.clone())
        } else {
            None
        };

        let matching_user_group_ids = self
            .resolve_matching_user_group_ids(policy, message, channel_name.clone())
            .await?;

        let decision = policy.decide(SlackAuthorizationContext {
            channel_name: channel_name.as_deref(),
            user_id: &message.user,
            actor_is_bot: message.actor_is_bot,
            matching_user_group_ids: &matching_user_group_ids,
        });

        Ok(AuthorizationResolution {
            decision,
            channel_name,
        })
    }

    async fn lookup_channel_info(
        &self,
        message: &SlackMessage,
    ) -> Result<SlackChannelInfo, AuthorizationFailure> {
        self.slack_channel_lookup_port
            .lookup_channel_info(&message.channel)
            .await
            .map_err(|error| AuthorizationFailure {
                reason: CHANNEL_LOOKUP_FAILED_REASON.to_string(),
                channel_name: None,
                error,
            })
    }

    async fn resolve_matching_user_group_ids(
        &self,
        policy: &SlackAuthorizationPolicy,
        message: &SlackMessage,
        channel_name: Option<String>,
    ) -> Result<Vec<String>, AuthorizationFailure> {
        if !policy.has_actor_condition()
            || policy.is_direct_user_allowed(&message.user)
            || policy.is_bot_allowed(message.actor_is_bot)
            || !policy.has_user_group_condition()
        {
            return Ok(Vec::new());
        }

        for user_group_id in policy.user_group_ids() {
            let members = self
                .slack_user_group_membership_port
                .list_user_group_members(user_group_id)
                .await
                .map_err(|error| AuthorizationFailure {
                    reason: USER_GROUP_LOOKUP_FAILED_REASON.to_string(),
                    channel_name: channel_name.clone(),
                    error,
                })?;

            if members
                .iter()
                .any(|member_user_id| member_user_id == &message.user)
            {
                return Ok(vec![user_group_id.clone()]);
            }
        }

        Ok(Vec::new())
    }

    async fn deny_mention(
        &self,
        message: &SlackMessage,
        channel_name: Option<&str>,
        reason: &str,
    ) -> Result<(), PortError> {
        self.logger.info(
            "slack_mention_denied",
            string_log_meta([
                ("slackEventId", message.slack_event_id.clone()),
                ("channel", message.channel.clone()),
                ("channelName", channel_name.unwrap_or_default().to_string()),
                ("user", message.user.clone()),
                ("reason", reason.to_string()),
            ]),
        );

        if let Err(error) = self
            .slack_ephemeral_message_port
            .post_ephemeral_message(PostSlackEphemeralMessageInput {
                channel: message.channel.clone(),
                user: message.user.clone(),
                thread_ts: message.thread_ts.clone(),
                text: DENY_EPHEMERAL_MESSAGE.to_string(),
            })
            .await
        {
            self.logger.error(
                "slack_authorization_deny_notification_failed",
                string_log_meta([
                    ("slackEventId", message.slack_event_id.clone()),
                    ("channel", message.channel.clone()),
                    ("user", message.user.clone()),
                    ("error", error.message),
                ]),
            );
        }

        Ok(())
    }

    fn log_authorization_failure(&self, message: &SlackMessage, failure: &AuthorizationFailure) {
        self.logger.error(
            "slack_authorization_failed",
            string_log_meta([
                ("slackEventId", message.slack_event_id.clone()),
                ("channel", message.channel.clone()),
                (
                    "channelName",
                    failure
                        .channel_name
                        .as_deref()
                        .unwrap_or_default()
                        .to_string(),
                ),
                ("user", message.user.clone()),
                (
                    "error",
                    format!("{}: {}", failure.reason, failure.error.message),
                ),
            ]),
        );
    }
}

impl SlackMentionAuthorizationGate {
    pub fn new(
        authorization_service: SlackMentionAuthorizationService,
        next_handler: Arc<dyn SlackMessageHandlerPort>,
    ) -> Self {
        Self {
            authorization_service,
            next_handler,
        }
    }
}

#[async_trait]
impl SlackMessageHandlerPort for SlackMentionAuthorizationGate {
    async fn handle(&self, message: SlackMessage) -> Result<(), PortError> {
        if message.trigger != SlackTriggerType::AppMention {
            return self.next_handler.handle(message).await;
        }

        match self.authorization_service.authorize(&message).await? {
            SlackMentionAuthorizationOutcome::Allowed => self.next_handler.handle(message).await,
            SlackMentionAuthorizationOutcome::Denied => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use reili_core::logger::LogEntry;
    use reili_core::messaging::slack::{
        MockSlackChannelLookupPort, MockSlackEphemeralMessagePort, MockSlackMessageHandlerPort,
        MockSlackUserGroupMembershipPort, PostSlackEphemeralMessageInput, SlackAuthorizationPolicy,
        SlackChannelInfo, SlackChannelLookupPort, SlackEphemeralMessagePort, SlackMessage,
        SlackMessageHandlerPort, SlackTriggerType, SlackUserGroupMembershipPort,
    };

    use super::{
        PortError, SlackMentionAuthorizationGate, SlackMentionAuthorizationService, TaskLogger,
    };

    #[derive(Default)]
    struct NoopLogger;

    impl TaskLogger for NoopLogger {
        fn log(&self, _entry: LogEntry) {}
    }

    struct TestContext {
        gate: SlackMentionAuthorizationGate,
        downstream_calls: Arc<Mutex<Vec<SlackMessage>>>,
        channel_lookup_calls: Arc<Mutex<Vec<String>>>,
        user_group_calls: Arc<Mutex<Vec<String>>>,
        ephemeral_calls: Arc<Mutex<Vec<PostSlackEphemeralMessageInput>>>,
    }

    fn create_gate(input: CreateGateInput) -> TestContext {
        let downstream_calls = Arc::new(Mutex::new(Vec::new()));
        let channel_lookup_calls = Arc::new(Mutex::new(Vec::new()));
        let user_group_calls = Arc::new(Mutex::new(Vec::new()));
        let ephemeral_calls = Arc::new(Mutex::new(Vec::new()));

        let mut next_handler = MockSlackMessageHandlerPort::new();
        let downstream_calls_mock = Arc::clone(&downstream_calls);
        next_handler
            .expect_handle()
            .returning(move |message: SlackMessage| {
                downstream_calls_mock
                    .lock()
                    .expect("lock downstream calls")
                    .push(message);
                Ok(())
            });

        let mut slack_channel_lookup_port = MockSlackChannelLookupPort::new();
        let channel_lookup_calls_mock = Arc::clone(&channel_lookup_calls);
        let channel_lookup_result = input.channel_lookup_result.clone();
        slack_channel_lookup_port
            .expect_lookup_channel_info()
            .returning(move |channel_id: &str| {
                channel_lookup_calls_mock
                    .lock()
                    .expect("lock channel lookup calls")
                    .push(channel_id.to_string());
                channel_lookup_result.clone()
            });

        let mut slack_user_group_membership_port = MockSlackUserGroupMembershipPort::new();
        let user_group_calls_mock = Arc::clone(&user_group_calls);
        let members_by_group = input.members_by_group.clone();
        slack_user_group_membership_port
            .expect_list_user_group_members()
            .returning(move |user_group_id: &str| {
                user_group_calls_mock
                    .lock()
                    .expect("lock user group calls")
                    .push(user_group_id.to_string());
                members_by_group
                    .get(user_group_id)
                    .cloned()
                    .unwrap_or_else(|| Ok(Vec::new()))
            });

        let mut slack_ephemeral_message_port = MockSlackEphemeralMessagePort::new();
        let ephemeral_calls_mock = Arc::clone(&ephemeral_calls);
        let ephemeral_result = input.ephemeral_result.clone();
        slack_ephemeral_message_port
            .expect_post_ephemeral_message()
            .returning(move |input: PostSlackEphemeralMessageInput| {
                ephemeral_calls_mock
                    .lock()
                    .expect("lock ephemeral message calls")
                    .push(input);
                ephemeral_result.clone()
            });

        let service = SlackMentionAuthorizationService::new(
            input.policy,
            Arc::new(slack_channel_lookup_port) as Arc<dyn SlackChannelLookupPort>,
            Arc::new(slack_user_group_membership_port) as Arc<dyn SlackUserGroupMembershipPort>,
            Arc::new(slack_ephemeral_message_port) as Arc<dyn SlackEphemeralMessagePort>,
            Arc::new(NoopLogger),
        );
        let gate = SlackMentionAuthorizationGate::new(
            service,
            Arc::new(next_handler) as Arc<dyn SlackMessageHandlerPort>,
        );

        TestContext {
            gate,
            downstream_calls,
            channel_lookup_calls,
            user_group_calls,
            ephemeral_calls,
        }
    }

    struct CreateGateInput {
        policy: SlackAuthorizationPolicy,
        channel_lookup_result: Result<SlackChannelInfo, PortError>,
        members_by_group: HashMap<String, Result<Vec<String>, PortError>>,
        ephemeral_result: Result<(), PortError>,
    }

    fn default_input(policy: SlackAuthorizationPolicy) -> CreateGateInput {
        CreateGateInput {
            policy,
            channel_lookup_result: Ok(channel_info("alerts-prod", false)),
            members_by_group: HashMap::new(),
            ephemeral_result: Ok(()),
        }
    }

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

    fn allow_all_policy() -> SlackAuthorizationPolicy {
        create_policy(None, None, None)
    }

    fn to_strings(values: Vec<&str>) -> Vec<String> {
        values.into_iter().map(ToString::to_string).collect()
    }

    fn channel_info(name: &str, is_private: bool) -> SlackChannelInfo {
        SlackChannelInfo {
            name: name.to_string(),
            is_private,
        }
    }

    fn create_message(trigger: SlackTriggerType) -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: Some("T001".to_string()),
            action_token: None,
            trigger,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            actor_is_bot: false,
            text: "<@U-BOT> investigate".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            ts: "1710000000.000001".to_string(),
            thread_ts: Some("1710000000.000000".to_string()),
        }
    }

    #[tokio::test]
    async fn delegates_to_downstream_only_when_authorized() {
        let context = create_gate(default_input(create_policy(
            Some(vec!["alerts-*"]),
            Some(vec!["U001"]),
            None,
        )));

        context
            .gate
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("authorize mention");

        assert_eq!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .len(),
            1
        );
        assert!(
            context
                .ephemeral_calls
                .lock()
                .expect("lock ephemeral calls")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn denies_without_calling_downstream_and_posts_ephemeral_message() {
        let mut input = default_input(create_policy(Some(vec!["alerts-*"]), None, None));
        input.channel_lookup_result = Ok(channel_info("deploy-prod", false));
        let context = create_gate(input);

        context
            .gate
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("authorize mention");

        assert!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .is_empty()
        );
        assert_eq!(
            context
                .ephemeral_calls
                .lock()
                .expect("lock ephemeral calls")
                .clone(),
            vec![PostSlackEphemeralMessageInput {
                channel: "C001".to_string(),
                user: "U001".to_string(),
                thread_ts: Some("1710000000.000000".to_string()),
                text: super::DENY_EPHEMERAL_MESSAGE.to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn posts_channel_ephemeral_message_without_thread_ts_for_root_mentions() {
        let mut input = default_input(create_policy(Some(vec!["alerts-*"]), None, None));
        input.channel_lookup_result = Ok(channel_info("deploy-prod", false));
        let context = create_gate(input);
        let mut message = create_message(SlackTriggerType::AppMention);
        message.thread_ts = None;

        context
            .gate
            .handle(message)
            .await
            .expect("authorize mention");

        assert_eq!(
            context
                .ephemeral_calls
                .lock()
                .expect("lock ephemeral calls")
                .clone(),
            vec![PostSlackEphemeralMessageInput {
                channel: "C001".to_string(),
                user: "U001".to_string(),
                thread_ts: None,
                text: super::DENY_EPHEMERAL_MESSAGE.to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn does_not_call_downstream_when_deny_notification_fails() {
        let mut input = default_input(create_policy(Some(vec!["alerts-*"]), None, None));
        input.channel_lookup_result = Ok(channel_info("deploy-prod", false));
        input.ephemeral_result = Err(PortError::new("ephemeral failed"));
        let context = create_gate(input);

        context
            .gate
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("authorization should swallow deny notification failures");

        assert!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .is_empty()
        );
        assert_eq!(
            context
                .ephemeral_calls
                .lock()
                .expect("lock ephemeral calls")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn skips_user_group_lookup_when_direct_user_id_matches() {
        let context = create_gate(default_input(create_policy(
            None,
            Some(vec!["U001"]),
            Some(vec!["S001"]),
        )));

        context
            .gate
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("authorize mention");

        assert!(
            context
                .user_group_calls
                .lock()
                .expect("lock user group calls")
                .is_empty()
        );
        assert_eq!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn delegates_bot_actor_when_bot_actors_are_allowed_without_user_group_lookup() {
        let context = create_gate(default_input(create_policy_with_allow_bot(
            Some(vec!["alerts-*"]),
            None,
            Some(vec!["S001"]),
            true,
        )));
        let mut message = create_message(SlackTriggerType::AppMention);
        message.user = "U-BOT-ACTOR".to_string();
        message.actor_is_bot = true;

        context
            .gate
            .handle(message)
            .await
            .expect("authorize bot mention");

        assert!(
            context
                .user_group_calls
                .lock()
                .expect("lock user group calls")
                .is_empty()
        );
        assert_eq!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn denies_bot_actor_when_bot_actors_are_not_allowed() {
        let context = create_gate(default_input(create_policy(
            Some(vec!["alerts-*"]),
            Some(vec!["U-HUMAN"]),
            None,
        )));
        let mut message = create_message(SlackTriggerType::AppMention);
        message.user = "U-BOT-ACTOR".to_string();
        message.actor_is_bot = true;

        context
            .gate
            .handle(message)
            .await
            .expect("authorize bot mention");

        assert!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .is_empty()
        );
        assert_eq!(
            context
                .ephemeral_calls
                .lock()
                .expect("lock ephemeral calls")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn delegates_when_conditions_are_not_configured_after_private_channel_check() {
        let context = create_gate(default_input(create_policy(None, None, None)));

        context
            .gate
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("authorize mention");

        assert_eq!(
            context
                .channel_lookup_calls
                .lock()
                .expect("lock channel lookup calls")
                .clone(),
            vec!["C001".to_string()]
        );
        assert!(
            context
                .user_group_calls
                .lock()
                .expect("lock user group calls")
                .is_empty()
        );
        assert_eq!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn delegates_public_app_mentions_when_policy_has_no_conditions() {
        let context = create_gate(CreateGateInput {
            policy: allow_all_policy(),
            channel_lookup_result: Ok(channel_info("alerts-prod", false)),
            members_by_group: HashMap::new(),
            ephemeral_result: Ok(()),
        });

        context
            .gate
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("handle mention");

        assert_eq!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .len(),
            1
        );
        assert_eq!(
            context
                .channel_lookup_calls
                .lock()
                .expect("lock channel lookup calls")
                .clone(),
            vec!["C001".to_string()]
        );
        assert!(
            context
                .ephemeral_calls
                .lock()
                .expect("lock ephemeral calls")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn denies_private_channel_app_mentions_even_with_allow_all_policy() {
        let context = create_gate(CreateGateInput {
            policy: allow_all_policy(),
            channel_lookup_result: Ok(channel_info("private-alerts", true)),
            members_by_group: HashMap::new(),
            ephemeral_result: Ok(()),
        });

        context
            .gate
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("handle mention");

        assert!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .is_empty()
        );
        assert_eq!(
            context
                .channel_lookup_calls
                .lock()
                .expect("lock channel lookup calls")
                .clone(),
            vec!["C001".to_string()]
        );
        assert_eq!(
            context
                .ephemeral_calls
                .lock()
                .expect("lock ephemeral calls")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn denies_empty_channel_allowlist_after_private_channel_check() {
        let context = create_gate(default_input(create_policy(
            Some(Vec::<&str>::new()),
            None,
            None,
        )));

        context
            .gate
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("authorize mention");

        assert!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .is_empty()
        );
        assert_eq!(
            context
                .channel_lookup_calls
                .lock()
                .expect("lock channel lookup calls")
                .clone(),
            vec!["C001".to_string()]
        );
        assert_eq!(
            context
                .ephemeral_calls
                .lock()
                .expect("lock ephemeral calls")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn fails_closed_when_channel_lookup_fails() {
        let mut input = default_input(create_policy(Some(vec!["alerts-*"]), None, None));
        input.channel_lookup_result = Err(PortError::new("channel lookup failed"));
        let context = create_gate(input);

        context
            .gate
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("authorization should fail closed");

        assert!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .is_empty()
        );
        assert_eq!(
            context
                .ephemeral_calls
                .lock()
                .expect("lock ephemeral calls")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn denies_when_private_channel_metadata_scope_is_missing() {
        let context = create_gate(CreateGateInput {
            policy: allow_all_policy(),
            channel_lookup_result: Err(PortError::service_error(
                "missing_scope",
                "Slack API returned error: method=conversations.info error=missing_scope",
            )),
            members_by_group: HashMap::new(),
            ephemeral_result: Ok(()),
        });

        context
            .gate
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("authorization should fail closed");

        assert!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .is_empty()
        );
        assert_eq!(
            context
                .channel_lookup_calls
                .lock()
                .expect("lock channel lookup calls")
                .clone(),
            vec!["C001".to_string()]
        );
        assert_eq!(
            context
                .ephemeral_calls
                .lock()
                .expect("lock ephemeral calls")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn fails_closed_when_user_group_lookup_fails() {
        let mut input = default_input(create_policy(None, None, Some(vec!["S001"])));
        input.members_by_group.insert(
            "S001".to_string(),
            Err(PortError::new("user group lookup failed")),
        );
        let context = create_gate(input);

        context
            .gate
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("authorization should fail closed");

        assert!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .is_empty()
        );
        assert_eq!(
            context
                .ephemeral_calls
                .lock()
                .expect("lock ephemeral calls")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn delegates_non_app_mentions_without_authorization_lookup() {
        let context = create_gate(default_input(create_policy(
            Some(vec!["alerts-*"]),
            None,
            Some(vec!["S001"]),
        )));

        context
            .gate
            .handle(create_message(SlackTriggerType::Message))
            .await
            .expect("handle message");

        assert!(
            context
                .channel_lookup_calls
                .lock()
                .expect("lock channel lookup calls")
                .is_empty()
        );
        assert!(
            context
                .user_group_calls
                .lock()
                .expect("lock user group calls")
                .is_empty()
        );
        assert_eq!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .len(),
            1
        );
    }
}
