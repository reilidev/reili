use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeMode {
    Ingress,
    Worker,
}

#[derive(Debug, Error)]
enum MainRunError {
    #[error("{0}")]
    Cli(#[from] CliArgError),
    #[error("{0}")]
    Ingress(#[from] sre_runtime::ingress::IngressRunError),
    #[error("{0}")]
    Worker(#[from] sre_runtime::worker::WorkerRunError),
}

#[derive(Debug, Error, PartialEq, Eq)]
enum CliArgError {
    #[error("Missing required argument: --mode ingress|worker")]
    MissingMode,
    #[error("Invalid --mode value: {value}")]
    InvalidMode { value: String },
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), MainRunError> {
    let mode = parse_runtime_mode(std::env::args().collect())?;

    match mode {
        RuntimeMode::Ingress => sre_runtime::ingress::run_ingress().await?,
        RuntimeMode::Worker => sre_runtime::worker::run_worker().await?,
    }

    Ok(())
}

fn parse_runtime_mode(args: Vec<String>) -> Result<RuntimeMode, CliArgError> {
    let mode_value = parse_mode_argument(&args).ok_or(CliArgError::MissingMode)?;

    parse_mode_value(mode_value)
}

fn parse_mode_argument(args: &[String]) -> Option<&str> {
    let mut index = 0_usize;
    while index < args.len() {
        let argument = args[index].as_str();
        if argument == "--mode" {
            if let Some(value) = args.get(index + 1) {
                return Some(value.as_str());
            }
            return None;
        }

        if let Some(value) = argument.strip_prefix("--mode=") {
            return Some(value);
        }

        index += 1;
    }

    None
}

fn parse_mode_value(value: &str) -> Result<RuntimeMode, CliArgError> {
    match value {
        "ingress" => Ok(RuntimeMode::Ingress),
        "worker" => Ok(RuntimeMode::Worker),
        _ => Err(CliArgError::InvalidMode {
            value: value.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{CliArgError, RuntimeMode, parse_runtime_mode};

    #[test]
    fn parses_mode_as_split_arguments() {
        let args = vec![
            "sre_runtime".to_string(),
            "--mode".to_string(),
            "ingress".to_string(),
        ];

        let mode = parse_runtime_mode(args).expect("parse mode");
        assert_eq!(mode, RuntimeMode::Ingress);
    }

    #[test]
    fn parses_mode_as_equals_argument() {
        let args = vec!["sre_runtime".to_string(), "--mode=worker".to_string()];

        let mode = parse_runtime_mode(args).expect("parse mode");
        assert_eq!(mode, RuntimeMode::Worker);
    }

    #[test]
    fn returns_error_for_missing_mode() {
        let args = vec!["sre_runtime".to_string()];

        let error = parse_runtime_mode(args).expect_err("missing mode should fail");
        assert_eq!(error, CliArgError::MissingMode);
    }

    #[test]
    fn returns_error_for_invalid_mode() {
        let args = vec![
            "sre_runtime".to_string(),
            "--mode".to_string(),
            "batch".to_string(),
        ];

        let error = parse_runtime_mode(args).expect_err("invalid mode should fail");
        assert_eq!(
            error,
            CliArgError::InvalidMode {
                value: "batch".to_string(),
            }
        );
    }
}
