use async_trait::async_trait;

use super::{
    SlackAppendStreamInput, SlackStartStreamInput, SlackStartStreamOutput, SlackStopStreamInput,
};

#[async_trait]
pub(crate) trait SlackProgressStreamApiPort: Send + Sync {
    async fn start(
        &self,
        input: SlackStartStreamInput,
    ) -> Result<SlackStartStreamOutput, reili_core::error::PortError>;
    async fn append(
        &self,
        input: SlackAppendStreamInput,
    ) -> Result<(), reili_core::error::PortError>;
    async fn stop(&self, input: SlackStopStreamInput) -> Result<(), reili_core::error::PortError>;
}
