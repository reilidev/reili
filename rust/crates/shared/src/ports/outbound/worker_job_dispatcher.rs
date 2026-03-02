use async_trait::async_trait;

use crate::{errors::PortError, types::InvestigationJob};

#[async_trait]
pub trait WorkerJobDispatcherPort: Send + Sync {
    async fn dispatch(&self, job: InvestigationJob) -> Result<(), PortError>;
}
