mod descriptor;
mod error;
mod factory;
mod prompt_fact;
mod set;

pub use descriptor::{ConnectorDescriptor, SubAgentPromptContext};
pub use error::ConnectorPrepareError;
pub use factory::{ConnectorFactory, PreparedConnector};
pub use prompt_fact::ConnectorPromptFact;
pub use set::ConnectorSet;
