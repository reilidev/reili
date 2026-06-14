use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    AutoResponseContextMessage, AutoResponseJudgeInput, AutoResponseJudgePort,
    FetchSlackThreadHistoryInput, SlackAuthorizationContext, SlackAuthorizationDecision,
    SlackAuthorizationPolicy, SlackChannelInfo, SlackChannelLookupPort, SlackChannelNamePattern,
    SlackMessage, SlackMessageHandlerPort, SlackThreadHistoryPort, SlackUserGroupMembershipPort,
};

use crate::{TaskLogger, string_log_meta};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackAutoResponsePolicy {
    pub names: Vec<SlackChannelNamePattern>,
    /// Judge policy override for these channels; `None` uses the judge's
    /// built-in policy.
    pub policy: Option<String>,
}

impl SlackAutoResponsePolicy {
    fn matches(&self, channel_name: &str) -> bool {
        self.names
            .iter()
            .any(|pattern| pattern.matches(channel_name))
    }
}

pub struct SlackAutoResponseGateDeps {
    pub channels: Vec<SlackAutoResponsePolicy>,
    pub actor_policy: SlackAuthorizationPolicy,
    pub channel_lookup_port: Arc<dyn SlackChannelLookupPort>,
    pub user_group_membership_port: Arc<dyn SlackUserGroupMembershipPort>,
    pub thread_history_port: Arc<dyn SlackThreadHistoryPort>,
    pub judge_port: Arc<dyn AutoResponseJudgePort>,
    pub language: String,
    pub next_handler: Arc<dyn SlackMessageHandlerPort>,
    pub logger: Arc<dyn TaskLogger>,
}

pub struct SlackAutoResponseGate {
    deps: SlackAutoResponseGateDeps,
}

impl SlackAutoResponseGate {
    pub fn new(deps: SlackAutoResponseGateDeps) -> Self {
        Self { deps }
    }

    fn discard(&self, message: &SlackMessage, channel_name: Option<&str>, reason: &str) {
        self.deps.logger.info(
            "slack_auto_response_discarded",
            string_log_meta([
                ("slackEventId", message.slack_event_id.clone()),
                ("channel", message.channel.clone()),
                ("channelName", channel_name.unwrap_or_default().to_string()),
                ("user", message.user.clone()),
            ]),
        );
        self.deps.logger.debug(
            "slack_auto_response_discard_reason",
            string_log_meta([
                ("slackEventId", message.slack_event_id.clone()),
                ("reason", reason.to_string()),
            ]),
        );
    }

    fn warn_failure(&self, message: &SlackMessage, reason: &str, error: &PortError) {
        self.deps.logger.warn(
            "slack_auto_response_failed",
            string_log_meta([
                ("slackEventId", message.slack_event_id.clone()),
                ("channel", message.channel.clone()),
                ("user", message.user.clone()),
                ("reason", reason.to_string()),
                ("error", error.message.clone()),
            ]),
        );
    }

    fn matching_policy(&self, channel_name: &str) -> Option<&SlackAutoResponsePolicy> {
        self.deps
            .channels
            .iter()
            .find(|policy| policy.matches(channel_name))
    }

    async fn is_actor_authorized(
        &self,
        message: &SlackMessage,
        channel_info: &SlackChannelInfo,
    ) -> Result<bool, PortError> {
        let matching_user_group_ids = self.resolve_matching_user_group_ids(message).await?;
        let decision = self.deps.actor_policy.decide(SlackAuthorizationContext {
            channel_name: Some(&channel_info.name),
            user_id: &message.user,
            actor_is_bot: message.actor_is_bot,
            matching_user_group_ids: &matching_user_group_ids,
        });

        Ok(decision == SlackAuthorizationDecision::Allow)
    }

    async fn resolve_matching_user_group_ids(
        &self,
        message: &SlackMessage,
    ) -> Result<Vec<String>, PortError> {
        let policy = &self.deps.actor_policy;
        if !policy.has_actor_condition()
            || policy.is_direct_user_allowed(&message.user)
            || policy.is_bot_allowed(message.actor_is_bot)
            || !policy.has_user_group_condition()
        {
            return Ok(Vec::new());
        }

        for user_group_id in policy.user_group_ids() {
            let members = self
                .deps
                .user_group_membership_port
                .list_user_group_members(user_group_id)
                .await?;

            if members
                .iter()
                .any(|member_user_id| member_user_id == &message.user)
            {
                return Ok(vec![user_group_id.clone()]);
            }
        }

        Ok(Vec::new())
    }

    async fn fetch_thread_context(
        &self,
        message: &SlackMessage,
    ) -> Vec<AutoResponseContextMessage> {
        let Some(thread_ts) = message.thread_ts.as_deref() else {
            return Vec::new();
        };
        if thread_ts == message.ts {
            return Vec::new();
        }

        match self
            .deps
            .thread_history_port
            .fetch_thread_history(FetchSlackThreadHistoryInput {
                channel: message.channel.clone(),
                thread_ts: thread_ts.to_string(),
            })
            .await
        {
            Ok(history) => history
                .iter()
                .filter(|thread_message| thread_message.ts != message.ts)
                .map(|thread_message| AutoResponseContextMessage {
                    ts: thread_message.ts.clone(),
                    user: thread_message.posted_by().to_string(),
                    text: thread_message.rendered_text(),
                })
                .collect(),
            Err(error) => {
                self.warn_failure(message, "thread_history_fetch_failed", &error);
                Vec::new()
            }
        }
    }
}

#[async_trait]
impl SlackMessageHandlerPort for SlackAutoResponseGate {
    async fn handle(&self, message: SlackMessage) -> Result<(), PortError> {
        let channel_info = match self
            .deps
            .channel_lookup_port
            .lookup_channel_info(&message.channel)
            .await
        {
            Ok(channel_info) => channel_info,
            Err(error) => {
                self.warn_failure(&message, "channel_lookup_failed", &error);
                return Ok(());
            }
        };

        if channel_info.is_private {
            self.discard(&message, Some(&channel_info.name), "private_channel");
            return Ok(());
        }

        let Some(policy) = self.matching_policy(&channel_info.name) else {
            return Ok(());
        };
        let judge_policy = policy.policy.clone();

        match self.is_actor_authorized(&message, &channel_info).await {
            Ok(true) => {}
            Ok(false) => {
                self.discard(&message, Some(&channel_info.name), "actor_not_allowed");
                return Ok(());
            }
            Err(error) => {
                self.warn_failure(&message, "user_group_membership_lookup_failed", &error);
                return Ok(());
            }
        }

        let thread_context = self.fetch_thread_context(&message).await;
        let decision = match self
            .deps
            .judge_port
            .judge(AutoResponseJudgeInput {
                policy: judge_policy,
                message_text: message.rendered_text(),
                thread_context,
                language: self.deps.language.clone(),
            })
            .await
        {
            Ok(decision) => decision,
            Err(error) => {
                self.warn_failure(&message, "judge_failed", &error);
                return Ok(());
            }
        };

        if !decision.respond {
            self.discard(
                &message,
                Some(&channel_info.name),
                &format!(
                    "judge_declined: {}",
                    decision.reason.as_deref().unwrap_or_default()
                ),
            );
            return Ok(());
        }

        self.deps.logger.info(
            "slack_auto_response_accepted",
            string_log_meta([
                ("slackEventId", message.slack_event_id.clone()),
                ("channel", message.channel.clone()),
                ("channelName", channel_info.name.clone()),
                ("user", message.user.clone()),
            ]),
        );
        self.deps.logger.debug(
            "slack_auto_response_accepted",
            string_log_meta([
                ("slackEventId", message.slack_event_id.clone()),
                ("channel", message.channel.clone()),
                ("channelName", channel_info.name.clone()),
                ("user", message.user.clone()),
                ("reason", decision.reason.unwrap_or_default()),
            ]),
        );

        self.deps.next_handler.handle(message).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::error::PortError;
    use reili_core::logger::LogEntry;
    use reili_core::messaging::slack::{
        AutoResponseJudgeDecision, AutoResponseJudgeInput, AutoResponseJudgePort,
        MockAutoResponseJudgePort, MockSlackChannelLookupPort, MockSlackMessageHandlerPort,
        MockSlackThreadHistoryPort, MockSlackUserGroupMembershipPort, SlackAuthorizationPolicy,
        SlackChannelInfo, SlackChannelLookupPort, SlackChannelNamePattern, SlackMessage,
        SlackMessageHandlerPort, SlackThreadHistoryPort, SlackThreadMessage, SlackTriggerType,
        SlackUserGroupMembershipPort,
    };

    use super::{
        SlackAutoResponseGate, SlackAutoResponseGateDeps, SlackAutoResponsePolicy, TaskLogger,
    };

    #[derive(Default)]
    struct NoopLogger;

    impl TaskLogger for NoopLogger {
        fn log(&self, _entry: LogEntry) {}
    }

    struct TestContext {
        gate: SlackAutoResponseGate,
        downstream_calls: Arc<Mutex<Vec<SlackMessage>>>,
        judge_calls: Arc<Mutex<Vec<AutoResponseJudgeInput>>>,
        thread_history_calls: Arc<Mutex<Vec<String>>>,
    }

    struct CreateGateInput {
        channels: Vec<SlackAutoResponsePolicy>,
        actor_policy: SlackAuthorizationPolicy,
        channel_lookup_result: Result<SlackChannelInfo, PortError>,
        thread_history_result: Result<Vec<SlackThreadMessage>, PortError>,
        judge_result: Result<AutoResponseJudgeDecision, PortError>,
    }

    fn default_input() -> CreateGateInput {
        CreateGateInput {
            channels: vec![alerts_policy()],
            actor_policy: SlackAuthorizationPolicy::new(None, None, None, false),
            channel_lookup_result: Ok(channel_info("alerts-prod", false)),
            thread_history_result: Ok(Vec::new()),
            judge_result: Ok(AutoResponseJudgeDecision {
                respond: true,
                reason: Some("incident signal".to_string()),
            }),
        }
    }

    fn patterns(names: &[&str]) -> Vec<SlackChannelNamePattern> {
        names
            .iter()
            .map(|name| SlackChannelNamePattern::new((*name).to_string()))
            .collect()
    }

    fn alerts_policy() -> SlackAutoResponsePolicy {
        SlackAutoResponsePolicy {
            names: patterns(&["alerts-*"]),
            policy: Some("React to production incidents.".to_string()),
        }
    }

    fn channel_info(name: &str, is_private: bool) -> SlackChannelInfo {
        SlackChannelInfo {
            name: name.to_string(),
            is_private,
        }
    }

    fn create_gate(input: CreateGateInput) -> TestContext {
        let downstream_calls = Arc::new(Mutex::new(Vec::new()));
        let judge_calls = Arc::new(Mutex::new(Vec::new()));
        let thread_history_calls = Arc::new(Mutex::new(Vec::new()));

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

        let mut channel_lookup_port = MockSlackChannelLookupPort::new();
        let channel_lookup_result = input.channel_lookup_result.clone();
        channel_lookup_port
            .expect_lookup_channel_info()
            .returning(move |_channel_id: &str| channel_lookup_result.clone());

        let mut user_group_membership_port = MockSlackUserGroupMembershipPort::new();
        user_group_membership_port
            .expect_list_user_group_members()
            .returning(|_user_group_id: &str| Ok(Vec::new()));

        let mut thread_history_port = MockSlackThreadHistoryPort::new();
        let thread_history_calls_mock = Arc::clone(&thread_history_calls);
        let thread_history_result = input.thread_history_result.clone();
        thread_history_port
            .expect_fetch_thread_history()
            .returning(move |fetch_input| {
                thread_history_calls_mock
                    .lock()
                    .expect("lock thread history calls")
                    .push(fetch_input.thread_ts);
                thread_history_result.clone()
            });

        let mut judge_port = MockAutoResponseJudgePort::new();
        let judge_calls_mock = Arc::clone(&judge_calls);
        let judge_result = input.judge_result.clone();
        judge_port
            .expect_judge()
            .returning(move |judge_input: AutoResponseJudgeInput| {
                judge_calls_mock
                    .lock()
                    .expect("lock judge calls")
                    .push(judge_input);
                judge_result.clone()
            });

        let gate = SlackAutoResponseGate::new(SlackAutoResponseGateDeps {
            channels: input.channels,
            actor_policy: input.actor_policy,
            channel_lookup_port: Arc::new(channel_lookup_port) as Arc<dyn SlackChannelLookupPort>,
            user_group_membership_port: Arc::new(user_group_membership_port)
                as Arc<dyn SlackUserGroupMembershipPort>,
            thread_history_port: Arc::new(thread_history_port) as Arc<dyn SlackThreadHistoryPort>,
            judge_port: Arc::new(judge_port) as Arc<dyn AutoResponseJudgePort>,
            language: "English".to_string(),
            next_handler: Arc::new(next_handler) as Arc<dyn SlackMessageHandlerPort>,
            logger: Arc::new(NoopLogger),
        });

        TestContext {
            gate,
            downstream_calls,
            judge_calls,
            thread_history_calls,
        }
    }

    fn create_message() -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: Some("T001".to_string()),
            action_token: None,
            trigger: SlackTriggerType::Message,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            actor_is_bot: false,
            text: "error rate is spiking".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            ts: "1710000000.000001".to_string(),
            thread_ts: None,
        }
    }

    fn downstream_count(context: &TestContext) -> usize {
        context
            .downstream_calls
            .lock()
            .expect("lock downstream calls")
            .len()
    }

    fn judge_count(context: &TestContext) -> usize {
        context.judge_calls.lock().expect("lock judge calls").len()
    }

    #[tokio::test]
    async fn discards_private_channel_messages() {
        let mut input = default_input();
        input.channel_lookup_result = Ok(channel_info("alerts-prod", true));
        let context = create_gate(input);

        context
            .gate
            .handle(create_message())
            .await
            .expect("handle message");

        assert_eq!(downstream_count(&context), 0);
        assert_eq!(judge_count(&context), 0);
    }

    #[tokio::test]
    async fn discards_when_channel_name_does_not_match_any_policy() {
        let mut input = default_input();
        input.channel_lookup_result = Ok(channel_info("random-talk", false));
        let context = create_gate(input);

        context
            .gate
            .handle(create_message())
            .await
            .expect("handle message");

        assert_eq!(downstream_count(&context), 0);
        assert_eq!(judge_count(&context), 0);
    }

    #[tokio::test]
    async fn discards_bot_actor_when_bots_are_not_allowed() {
        let mut input = default_input();
        input.actor_policy = SlackAuthorizationPolicy::new(None, Some(Vec::new()), None, false);
        let context = create_gate(input);
        let mut message = create_message();
        message.user = "U-BOT-ACTOR".to_string();
        message.actor_is_bot = true;

        context.gate.handle(message).await.expect("handle message");

        assert_eq!(downstream_count(&context), 0);
        assert_eq!(judge_count(&context), 0);
    }

    #[tokio::test]
    async fn delegates_bot_actor_when_bots_are_allowed() {
        let mut input = default_input();
        input.actor_policy = SlackAuthorizationPolicy::new(None, None, None, true);
        let context = create_gate(input);
        let mut message = create_message();
        message.user = "U-BOT-ACTOR".to_string();
        message.actor_is_bot = true;

        context.gate.handle(message).await.expect("handle message");

        assert_eq!(downstream_count(&context), 1);
    }

    #[tokio::test]
    async fn discards_when_judge_declines() {
        let mut input = default_input();
        input.judge_result = Ok(AutoResponseJudgeDecision {
            respond: false,
            reason: Some("casual chat".to_string()),
        });
        let context = create_gate(input);

        context
            .gate
            .handle(create_message())
            .await
            .expect("handle message");

        assert_eq!(judge_count(&context), 1);
        assert_eq!(downstream_count(&context), 0);
    }

    #[tokio::test]
    async fn delegates_original_message_when_judge_accepts() {
        let context = create_gate(default_input());
        let message = create_message();

        context
            .gate
            .handle(message.clone())
            .await
            .expect("handle message");

        assert_eq!(judge_count(&context), 1);
        assert_eq!(
            context
                .downstream_calls
                .lock()
                .expect("lock downstream calls")
                .clone(),
            vec![message]
        );

        let judge_input = context.judge_calls.lock().expect("lock judge calls")[0].clone();
        assert_eq!(judge_input.policy, alerts_policy().policy);
        assert_eq!(judge_input.message_text, "error rate is spiking");
        assert!(judge_input.thread_context.is_empty());
        assert_eq!(judge_input.language, "English");
    }

    #[tokio::test]
    async fn uses_first_matching_policy() {
        let mut input = default_input();
        input.channels = vec![
            SlackAutoResponsePolicy {
                names: patterns(&["incidents"]),
                policy: Some("incidents policy".to_string()),
            },
            SlackAutoResponsePolicy {
                names: patterns(&["alerts-*"]),
                policy: Some("first alerts policy".to_string()),
            },
            SlackAutoResponsePolicy {
                names: patterns(&["alerts-prod"]),
                policy: Some("second alerts policy".to_string()),
            },
        ];
        let context = create_gate(input);

        context
            .gate
            .handle(create_message())
            .await
            .expect("handle message");

        let judge_input = context.judge_calls.lock().expect("lock judge calls")[0].clone();
        assert_eq!(judge_input.policy.as_deref(), Some("first alerts policy"));
    }

    #[tokio::test]
    async fn passes_missing_policy_through_to_judge() {
        let mut input = default_input();
        input.channels = vec![SlackAutoResponsePolicy {
            names: patterns(&["alerts-*"]),
            policy: None,
        }];
        let context = create_gate(input);

        context
            .gate
            .handle(create_message())
            .await
            .expect("handle message");

        let judge_input = context.judge_calls.lock().expect("lock judge calls")[0].clone();
        assert_eq!(judge_input.policy, None);
        assert_eq!(downstream_count(&context), 1);
    }

    #[tokio::test]
    async fn fails_closed_when_channel_lookup_fails() {
        let mut input = default_input();
        input.channel_lookup_result = Err(PortError::new("lookup failed"));
        let context = create_gate(input);

        context
            .gate
            .handle(create_message())
            .await
            .expect("handle message");

        assert_eq!(downstream_count(&context), 0);
        assert_eq!(judge_count(&context), 0);
    }

    #[tokio::test]
    async fn fails_silent_when_judge_fails() {
        let mut input = default_input();
        input.judge_result = Err(PortError::new("judge failed"));
        let context = create_gate(input);

        context
            .gate
            .handle(create_message())
            .await
            .expect("handle message");

        assert_eq!(judge_count(&context), 1);
        assert_eq!(downstream_count(&context), 0);
    }

    #[tokio::test]
    async fn includes_thread_history_in_judge_input_for_thread_replies() {
        let mut input = default_input();
        input.thread_history_result = Ok(vec![
            SlackThreadMessage {
                ts: "1710000000.000000".to_string(),
                user: Some("U002".to_string()),
                text: "deploy finished".to_string(),
                legacy_attachments: Vec::new(),
                files: Vec::new(),
                metadata: None,
            },
            SlackThreadMessage {
                ts: "1710000000.000001".to_string(),
                user: Some("U001".to_string()),
                text: "error rate is spiking".to_string(),
                legacy_attachments: Vec::new(),
                files: Vec::new(),
                metadata: None,
            },
        ]);
        let context = create_gate(input);
        let mut message = create_message();
        message.thread_ts = Some("1710000000.000000".to_string());

        context.gate.handle(message).await.expect("handle message");

        assert_eq!(
            context
                .thread_history_calls
                .lock()
                .expect("lock thread history calls")
                .clone(),
            vec!["1710000000.000000".to_string()]
        );

        let judge_input = context.judge_calls.lock().expect("lock judge calls")[0].clone();
        assert_eq!(judge_input.thread_context.len(), 1);
        assert_eq!(judge_input.thread_context[0].ts, "1710000000.000000");
        assert_eq!(judge_input.thread_context[0].user, "U002");
        assert_eq!(judge_input.thread_context[0].text, "deploy finished");
    }

    #[tokio::test]
    async fn skips_thread_history_for_top_level_messages() {
        let context = create_gate(default_input());

        context
            .gate
            .handle(create_message())
            .await
            .expect("handle message");

        assert!(
            context
                .thread_history_calls
                .lock()
                .expect("lock thread history calls")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn continues_without_context_when_thread_history_fetch_fails() {
        let mut input = default_input();
        input.thread_history_result = Err(PortError::new("history failed"));
        let context = create_gate(input);
        let mut message = create_message();
        message.thread_ts = Some("1710000000.000000".to_string());

        context.gate.handle(message).await.expect("handle message");

        assert_eq!(judge_count(&context), 1);
        let judge_input = context.judge_calls.lock().expect("lock judge calls")[0].clone();
        assert!(judge_input.thread_context.is_empty());
        assert_eq!(downstream_count(&context), 1);
    }
}
