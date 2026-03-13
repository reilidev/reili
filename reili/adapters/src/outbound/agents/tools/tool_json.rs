use serde::Serialize;
use reili_shared::errors::PortError;

pub fn to_json_string<T>(value: &T) -> Result<String, PortError>
where
    T: Serialize,
{
    serde_json::to_string(value)
        .map_err(|error| PortError::new(format!("Failed to serialize tool result: {error}")))
}
