use std::sync::Arc;

use super::error::ConnectorPrepareError;
use super::factory::{ConnectorFactory, PreparedConnector};

/// Ordered registry of connectors. The registration order is the lead tool order and the prompt
/// fact order, so it is kept deterministic.
#[derive(Clone, Default)]
pub struct ConnectorSet {
    factories: Vec<Arc<dyn ConnectorFactory>>,
}

impl ConnectorSet {
    #[must_use]
    pub fn new(factories: Vec<Arc<dyn ConnectorFactory>>) -> Self {
        Self { factories }
    }

    pub fn push(&mut self, factory: Arc<dyn ConnectorFactory>) {
        self.factories.push(factory);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.factories.len()
    }

    /// Connect every connector in registration order, preserving order in the result.
    pub async fn prepare_all(
        &self,
    ) -> Result<Vec<Arc<dyn PreparedConnector>>, ConnectorPrepareError> {
        let mut prepared = Vec::with_capacity(self.factories.len());
        for factory in &self.factories {
            prepared.push(factory.prepare().await?);
        }
        Ok(prepared)
    }
}
