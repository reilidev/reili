use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogApiRetryConfig {
    pub enabled: bool,
    pub max_retries: u32,
    pub backoff_base_seconds: u32,
    pub backoff_multiplier: u32,
}

#[cfg(test)]
mod tests {
    use super::DatadogApiRetryConfig;

    #[test]
    fn serializes_and_deserializes_retry_config() {
        let value = DatadogApiRetryConfig {
            enabled: true,
            max_retries: 3,
            backoff_base_seconds: 2,
            backoff_multiplier: 2,
        };

        let json = serde_json::to_string(&value).expect("serialize retry config");
        let restored: DatadogApiRetryConfig =
            serde_json::from_str(&json).expect("deserialize retry config");

        assert_eq!(restored, value);
    }
}
