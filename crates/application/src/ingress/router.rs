use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{SlackMessage, SlackMessageHandlerPort, SlackTriggerType};

pub struct SlackInboundRouter {
    mention_gate: Arc<dyn SlackMessageHandlerPort>,
    auto_response_gate: Option<Arc<dyn SlackMessageHandlerPort>>,
}

impl SlackInboundRouter {
    pub fn new(
        mention_gate: Arc<dyn SlackMessageHandlerPort>,
        auto_response_gate: Option<Arc<dyn SlackMessageHandlerPort>>,
    ) -> Self {
        Self {
            mention_gate,
            auto_response_gate,
        }
    }
}

#[async_trait]
impl SlackMessageHandlerPort for SlackInboundRouter {
    async fn handle(&self, message: SlackMessage) -> Result<(), PortError> {
        match message.trigger {
            SlackTriggerType::AppMention => self.mention_gate.handle(message).await,
            SlackTriggerType::Message => match &self.auto_response_gate {
                Some(gate) => gate.handle(message).await,
                None => Ok(()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::messaging::slack::{
        MockSlackMessageHandlerPort, SlackMessage, SlackMessageHandlerPort, SlackTriggerType,
    };

    use super::SlackInboundRouter;

    fn create_message(trigger: SlackTriggerType) -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: Some("T001".to_string()),
            action_token: None,
            trigger,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            actor_is_bot: false,
            text: "hello".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            ts: "1710000000.000001".to_string(),
            thread_ts: None,
        }
    }

    fn recording_handler() -> (
        Arc<dyn SlackMessageHandlerPort>,
        Arc<Mutex<Vec<SlackMessage>>>,
    ) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut handler = MockSlackMessageHandlerPort::new();
        let calls_mock = Arc::clone(&calls);
        handler
            .expect_handle()
            .returning(move |message: SlackMessage| {
                calls_mock.lock().expect("lock calls").push(message);
                Ok(())
            });

        (Arc::new(handler) as Arc<dyn SlackMessageHandlerPort>, calls)
    }

    #[tokio::test]
    async fn routes_app_mentions_to_mention_gate() {
        let (mention_gate, mention_calls) = recording_handler();
        let (auto_response_gate, auto_response_calls) = recording_handler();
        let router = SlackInboundRouter::new(mention_gate, Some(auto_response_gate));

        router
            .handle(create_message(SlackTriggerType::AppMention))
            .await
            .expect("handle mention");

        assert_eq!(mention_calls.lock().expect("lock calls").len(), 1);
        assert!(auto_response_calls.lock().expect("lock calls").is_empty());
    }

    #[tokio::test]
    async fn routes_messages_to_auto_response_gate() {
        let (mention_gate, mention_calls) = recording_handler();
        let (auto_response_gate, auto_response_calls) = recording_handler();
        let router = SlackInboundRouter::new(mention_gate, Some(auto_response_gate));

        router
            .handle(create_message(SlackTriggerType::Message))
            .await
            .expect("handle message");

        assert!(mention_calls.lock().expect("lock calls").is_empty());
        assert_eq!(auto_response_calls.lock().expect("lock calls").len(), 1);
    }

    #[tokio::test]
    async fn discards_messages_when_auto_response_gate_is_not_configured() {
        let (mention_gate, mention_calls) = recording_handler();
        let router = SlackInboundRouter::new(mention_gate, None);

        router
            .handle(create_message(SlackTriggerType::Message))
            .await
            .expect("handle message");

        assert!(mention_calls.lock().expect("lock calls").is_empty());
    }
}
