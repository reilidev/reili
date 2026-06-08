use reili_core::error::PortError;

/// Classification of a connector connection failure, mapped centrally by the caller.
#[derive(Debug)]
pub enum ConnectorPrepareError {
    /// Transport could not be established → maps to a permanent task error.
    ConnectionFailed { message: String },
    /// Any other failure → maps to a normal task failure.
    Other(PortError),
}

impl ConnectorPrepareError {
    /// Classify a [`PortError`] raised while preparing a connector.
    pub fn from_port_error(error: PortError) -> Self {
        if error.is_connection_failed() {
            Self::ConnectionFailed {
                message: error.message,
            }
        } else {
            Self::Other(error)
        }
    }
}
