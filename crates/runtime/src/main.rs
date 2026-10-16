use std::ffi::OsString;
use std::path::PathBuf;

use reili_runtime::app::{AppStartupOptions, run_app_with_options};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum StartupOptionsError {
    #[error("Missing value for `--config`")]
    MissingConfigPath,
    #[error("Unknown startup argument `{value}`")]
    UnknownArgument { value: String },
}

fn parse_startup_options<I>(args: I) -> Result<AppStartupOptions, StartupOptionsError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut config_path = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        if arg == "--config" {
            let Some(path) = args.next() else {
                return Err(StartupOptionsError::MissingConfigPath);
            };
            config_path = Some(PathBuf::from(path));
            continue;
        }

        return Err(StartupOptionsError::UnknownArgument {
            value: arg.to_string_lossy().into_owned(),
        });
    }

    Ok(AppStartupOptions { config_path })
}

#[tokio::main]
async fn main() {
    let startup_options = match parse_startup_options(std::env::args_os().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };

    if let Err(error) = run_app_with_options(startup_options).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::{AppStartupOptions, StartupOptionsError, parse_startup_options};

    #[test]
    fn parses_config_path_flag() {
        let options = parse_startup_options([
            OsString::from("--config"),
            OsString::from("/tmp/reili.toml"),
        ])
        .expect("parse startup options");

        assert_eq!(
            options,
            AppStartupOptions {
                config_path: Some(PathBuf::from("/tmp/reili.toml")),
            }
        );
    }

    #[test]
    fn rejects_missing_config_path_value() {
        let error = parse_startup_options([OsString::from("--config")]).expect_err("missing value");

        assert_eq!(error, StartupOptionsError::MissingConfigPath);
    }

    #[test]
    fn rejects_unknown_arguments() {
        let error = parse_startup_options([OsString::from("--unknown")]).expect_err("unknown arg");

        assert_eq!(
            error,
            StartupOptionsError::UnknownArgument {
                value: "--unknown".to_string(),
            }
        );
    }
}
