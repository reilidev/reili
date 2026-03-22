use crate::messaging::slack::{SlackMessage, SlackThreadMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvestigationRequest {
    pub trigger_message: SlackMessage,
    pub thread_messages: Vec<SlackThreadMessage>,
}
