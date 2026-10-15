use std::ffi::OsString;
use std::path::PathBuf;

use reili_runtime::app::{AppStartupOptions, run_app_with_options};
use reili_runtime::eval::{EvalOptions, run_eval};

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve(AppStartupOptions),
    Eval(EvalOptions),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum StartupOptionsError {
    #[error("Missing value for `--config`")]
    MissingConfigPath,
    #[error("Missing value for `{flag}`")]
    MissingValue { flag: String },
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

/// `reili eval [--replay] [--config PATH] [--fixtures DIR] [--baseline FILE] [--output FILE]
/// [--markdown FILE]`
fn parse_eval_options<I>(args: I) -> Result<EvalOptions, StartupOptionsError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = EvalOptions::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let flag = arg.to_string_lossy().into_owned();
        if flag == "--replay" {
            options.replay = true;
            continue;
        }
        let mut value = || {
            args.next()
                .map(PathBuf::from)
                .ok_or_else(|| StartupOptionsError::MissingValue { flag: flag.clone() })
        };
        match flag.as_str() {
            "--config" => options.config_path = Some(value()?),
            "--fixtures" => options.fixtures_dir = Some(value()?),
            "--baseline" => options.baseline = Some(value()?),
            "--output" => options.output = Some(value()?),
            "--markdown" => options.markdown = Some(value()?),
            _ => return Err(StartupOptionsError::UnknownArgument { value: flag }),
        }
    }
    Ok(options)
}

fn parse_command<I>(args: I) -> Result<Command, StartupOptionsError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter().peekable();
    if args.peek().is_some_and(|first| first == "eval") {
        args.next();
        return parse_eval_options(args).map(Command::Eval);
    }
    parse_startup_options(args).map(Command::Serve)
}

#[tokio::main]
async fn main() {
    let command = match parse_command(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };

    match command {
        Command::Serve(startup_options) => {
            if let Err(error) = run_app_with_options(startup_options).await {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        Command::Eval(options) => match run_eval(options).await {
            Ok(output) if output.succeeded() => {}
            Ok(_) => std::process::exit(2),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::{
        AppStartupOptions, Command, EvalOptions, StartupOptionsError, parse_command,
        parse_startup_options,
    };

    #[test]
    fn parses_eval_subcommand() {
        let command = parse_command([
            OsString::from("eval"),
            OsString::from("--replay"),
            OsString::from("--output"),
            OsString::from("out.json"),
        ])
        .expect("parse eval");

        assert_eq!(
            command,
            Command::Eval(EvalOptions {
                replay: true,
                output: Some(PathBuf::from("out.json")),
                ..EvalOptions::default()
            })
        );
        assert_eq!(
            parse_command([OsString::from("eval"), OsString::from("--baseline")]),
            Err(StartupOptionsError::MissingValue {
                flag: "--baseline".to_string()
            })
        );
        assert!(matches!(
            parse_command([OsString::from("--config"), OsString::from("r.toml")]),
            Ok(Command::Serve(_))
        ));
    }

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
