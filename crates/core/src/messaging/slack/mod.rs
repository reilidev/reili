pub mod control_message;
pub mod interaction;
pub mod interaction_handler;
pub mod message;
pub mod message_handler;
pub mod reaction;
pub mod thread_history;
pub mod thread_message;
pub mod thread_reply;

pub use control_message::{
    PostTaskControlMessageInput, PostTaskControlMessageOutput, SlackTaskControlMessagePort,
    SlackTaskControlState, UpdateTaskControlMessageInput,
};
pub use interaction::{SlackCancelJobInteraction, SlackInteraction};
pub use interaction_handler::SlackInteractionHandlerPort;
pub use message::{SlackMessage, SlackTriggerType};
pub use message_handler::SlackMessageHandlerPort;
pub use reaction::{AddSlackReactionInput, SlackReactionPort};
pub use thread_history::{FetchSlackThreadHistoryInput, SlackThreadHistoryPort};
pub use thread_message::{SlackMessageMetadata, SlackThreadMessage};
pub use thread_reply::{SlackThreadReplyInput, SlackThreadReplyPort};

#[cfg(any(test, feature = "test-support"))]
pub use control_message::MockSlackTaskControlMessagePort;
#[cfg(any(test, feature = "test-support"))]
pub use interaction_handler::MockSlackInteractionHandlerPort;
#[cfg(any(test, feature = "test-support"))]
pub use message_handler::MockSlackMessageHandlerPort;
#[cfg(any(test, feature = "test-support"))]
pub use reaction::MockSlackReactionPort;
#[cfg(any(test, feature = "test-support"))]
pub use thread_history::MockSlackThreadHistoryPort;
#[cfg(any(test, feature = "test-support"))]
pub use thread_reply::MockSlackThreadReplyPort;
