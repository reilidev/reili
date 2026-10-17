//! Versioned machine state. Backends store the state JSON verbatim and decode it through
//! [`decode_state_json`], so a Reili that is older than the state it reads refuses it instead of
//! overwriting fields it does not know.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

use super::item::{WORK_ITEM_SCHEMA_VERSION, WorkItemState};
use super::ledger::{OBJECTIVE_LEDGER_SCHEMA_VERSION, ObjectiveLedgerState};

pub trait VersionedState: Serialize + DeserializeOwned {
    /// Newest schema version this build reads and writes.
    const SCHEMA_VERSION: u32;
}

impl VersionedState for WorkItemState {
    const SCHEMA_VERSION: u32 = WORK_ITEM_SCHEMA_VERSION;
}

impl VersionedState for ObjectiveLedgerState {
    const SCHEMA_VERSION: u32 = OBJECTIVE_LEDGER_SCHEMA_VERSION;
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StateDecodeError {
    #[error("machine state is not valid: {0}")]
    Invalid(String),
    #[error("machine state has schema version {found}; this Reili supports up to {supported}")]
    Unsupported { found: u32, supported: u32 },
}

/// Decodes state written by this or an earlier schema version. Migrations from earlier versions
/// belong here once a version is retired.
pub fn decode_state_json<T: VersionedState>(json: &str) -> Result<T, StateDecodeError> {
    let value: Value =
        serde_json::from_str(json).map_err(|error| StateDecodeError::Invalid(error.to_string()))?;
    let found = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| StateDecodeError::Invalid("`schema_version` is missing".to_string()))?;
    if found > T::SCHEMA_VERSION {
        return Err(StateDecodeError::Unsupported {
            found,
            supported: T::SCHEMA_VERSION,
        });
    }
    serde_json::from_value(value).map_err(|error| StateDecodeError::Invalid(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{StateDecodeError, decode_state_json};
    use crate::work::{WorkItemState, test_support::sample_work_item};

    #[test]
    fn round_trips_the_current_version() {
        let item = sample_work_item("checkout:errors");
        let json = serde_json::to_string(&item).expect("encode");

        let decoded: WorkItemState = decode_state_json(&json).expect("decode");

        assert_eq!(decoded, item);
    }

    #[test]
    fn refuses_state_from_a_newer_schema() {
        let mut item = sample_work_item("checkout:errors");
        item.schema_version = 99;
        let json = serde_json::to_string(&item).expect("encode");

        let error = decode_state_json::<WorkItemState>(&json).expect_err("newer schema");

        assert_eq!(
            error,
            StateDecodeError::Unsupported {
                found: 99,
                supported: 1
            }
        );
    }

    #[test]
    fn reports_missing_version_as_invalid() {
        let error = decode_state_json::<WorkItemState>("{}").expect_err("invalid");

        assert!(matches!(error, StateDecodeError::Invalid(_)));
    }
}
