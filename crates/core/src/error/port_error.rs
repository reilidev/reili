use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortErrorKind {
    HttpStatus { status_code: u16 },
    InvalidInput,
    ConnectionFailed,
    InvalidResponse,
    ServiceError { error_code: String },
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct PortError {
    pub message: String,
    pub kind: PortErrorKind,
}

impl PortError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: PortErrorKind::Other,
        }
    }

    pub fn http_status(status_code: u16, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: PortErrorKind::HttpStatus { status_code },
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: PortErrorKind::InvalidInput,
        }
    }

    pub fn connection_failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: PortErrorKind::ConnectionFailed,
        }
    }

    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: PortErrorKind::InvalidResponse,
        }
    }

    pub fn service_error(error_code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: PortErrorKind::ServiceError {
                error_code: error_code.into(),
            },
        }
    }

    pub fn is_client_error(&self) -> bool {
        matches!(
            &self.kind,
            PortErrorKind::HttpStatus { status_code } if (400..500).contains(status_code)
        )
    }

    pub fn is_invalid_input(&self) -> bool {
        matches!(&self.kind, PortErrorKind::InvalidInput)
    }

    pub fn is_connection_failed(&self) -> bool {
        matches!(&self.kind, PortErrorKind::ConnectionFailed)
    }

    pub fn status_code(&self) -> Option<u16> {
        match &self.kind {
            PortErrorKind::HttpStatus { status_code } => Some(*status_code),
            _ => None,
        }
    }

    pub fn service_error_code(&self) -> Option<&str> {
        match &self.kind {
            PortErrorKind::ServiceError { error_code } => Some(error_code.as_str()),
            _ => None,
        }
    }

    pub fn is_service_error_code(&self, error_code: &str) -> bool {
        self.service_error_code() == Some(error_code)
    }
}

#[cfg(test)]
mod tests {
    use super::{PortError, PortErrorKind};

    #[test]
    fn new_defaults_to_other_kind() {
        let error = PortError::new("failed");

        assert_eq!(error.kind, PortErrorKind::Other);
    }

    #[test]
    fn http_status_exposes_client_error_helpers() {
        let error = PortError::http_status(422, "unprocessable");

        assert!(error.is_client_error());
        assert_eq!(error.status_code(), Some(422));
    }

    #[test]
    fn service_error_exposes_error_code() {
        let error = PortError::service_error("invalid_ts", "slack error");

        assert_eq!(error.service_error_code(), Some("invalid_ts"));
        assert!(error.is_service_error_code("invalid_ts"));
    }
}
