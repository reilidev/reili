//! `reili eval`: score investigations against replayable fixtures.
//!
//! - `--replay` (default in CI): recorded runs are replayed through the production report policy
//!   and scored. Deterministic, no credentials needed.
//! - live (no `--replay`): the configured AI backend investigates each fixture with its metric
//!   data in the brief and no live connectors, then the same scorer runs.
//!
//! Results are written as JSON (for baselines) and Markdown (for CI summaries/artifacts).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use reili_adapters::outbound::agents::ConnectorSet;
use reili_application::goal::evaluation::{
    BUNDLED_CASES, EvaluationCase, EvaluationComparison, EvaluationSummary, EvaluationThresholds,
    LiveEvaluationInput, Scorecard, compare_summaries, parse_case, render_markdown,
    replay_recorded_case, run_live_case, summarize,
};
use reili_core::error::PortError;
use reili_core::investigation::RunLimits;
use reili_core::messaging::slack::{
    SlackFileDownloadPort, SlackMessageSearchInput, SlackMessageSearchPort,
    SlackMessageSearchResult,
};
use reili_core::task::TaskResources;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bootstrap::{CreateTaskRunnerInput, create_agent_runners, create_task_logger};
use crate::config::{ConfigLoadOptions, load_eval_config};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvalOptions {
    pub config_path: Option<PathBuf>,
    pub fixtures_dir: Option<PathBuf>,
    pub replay: bool,
    pub baseline: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub markdown: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("Failed to read `{path}`: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Failed to write `{path}`: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{0}")]
    Case(String),
    #[error("{0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("{0}")]
    Bootstrap(#[from] crate::bootstrap::RuntimeBootstrapError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationOutput {
    pub mode: String,
    pub summary: EvaluationSummary,
    pub scorecards: Vec<Scorecard>,
    pub comparison: Option<EvaluationComparison>,
}

impl EvaluationOutput {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.summary.passed
            && self
                .comparison
                .as_ref()
                .is_none_or(|comparison| comparison.regressions.is_empty())
    }
}

fn read(path: &Path) -> Result<String, EvalError> {
    std::fs::read_to_string(path).map_err(|source| EvalError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn load_cases(fixtures_dir: Option<&Path>) -> Result<Vec<EvaluationCase>, EvalError> {
    let sources: Vec<String> = match fixtures_dir {
        None => BUNDLED_CASES.iter().map(ToString::to_string).collect(),
        Some(dir) => {
            let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
                .map_err(|source| EvalError::Read {
                    path: dir.to_path_buf(),
                    source,
                })?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "json")
                })
                .collect();
            paths.sort();
            paths
                .iter()
                .map(|path| read(path))
                .collect::<Result<_, _>>()?
        }
    };
    sources
        .iter()
        .map(|source| parse_case(source).map_err(|error| EvalError::Case(error.to_string())))
        .collect()
}

/// Runs an evaluation and writes the requested outputs. Returns the output for exit-code logic.
pub async fn run_eval(options: EvalOptions) -> Result<EvaluationOutput, EvalError> {
    let cases = load_cases(options.fixtures_dir.as_deref())?;
    let expectations: BTreeMap<String, bool> = cases
        .iter()
        .map(|case| (case.id.clone(), case.ground_truth.expect_findings))
        .collect();
    let scorecards = if options.replay {
        cases
            .iter()
            .map(|case| {
                replay_recorded_case(case).map_err(|error| EvalError::Case(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        run_live(&options, cases).await?
    };
    let summary = summarize(&scorecards, &expectations, EvaluationThresholds::default());
    let comparison = match &options.baseline {
        Some(path) => {
            let baseline: EvaluationOutput = serde_json::from_str(&read(path)?)
                .map_err(|error| EvalError::Case(format!("invalid baseline: {error}")))?;
            Some(compare_summaries(&baseline.summary, &summary))
        }
        None => None,
    };
    let output = EvaluationOutput {
        mode: if options.replay { "replay" } else { "live" }.to_string(),
        summary,
        scorecards,
        comparison,
    };
    let markdown = render_markdown(
        &output.summary,
        &output.scorecards,
        output.comparison.as_ref(),
    );
    if let Some(path) = &options.output {
        let json = serde_json::to_string_pretty(&output)
            .map_err(|error| EvalError::Case(error.to_string()))?;
        std::fs::write(path, json).map_err(|source| EvalError::Write {
            path: path.clone(),
            source,
        })?;
    }
    match &options.markdown {
        Some(path) => std::fs::write(path, &markdown).map_err(|source| EvalError::Write {
            path: path.clone(),
            source,
        })?,
        None => println!("{markdown}"),
    }
    Ok(output)
}

async fn run_live(
    options: &EvalOptions,
    cases: Vec<EvaluationCase>,
) -> Result<Vec<Scorecard>, EvalError> {
    let config = load_eval_config(&ConfigLoadOptions {
        explicit_path: options.config_path.clone(),
    })?;
    let runners = create_agent_runners(CreateTaskRunnerInput {
        llm_provider: &config.llm.provider,
        connectors: ConnectorSet::default(),
        language: config.language.clone(),
        additional_system_prompt: config.additional_system_prompt.clone(),
    })
    .await?;
    let logger = create_task_logger();
    let mut scorecards = Vec::new();
    for case in cases {
        scorecards.push(
            run_live_case(LiveEvaluationInput {
                case,
                runner: Arc::clone(&runners.investigation),
                resources: offline_resources(),
                limits: RunLimits {
                    max_tool_calls: 80,
                    max_llm_tokens: 1_000_000,
                    max_sub_agents: 6,
                    max_parallel_tools: 4,
                },
                logger: Arc::clone(&logger),
            })
            .await,
        );
    }
    Ok(scorecards)
}

/// Evaluation runs offline: Slack and the web are unavailable, metrics come from fixtures.
fn offline_resources() -> TaskResources {
    TaskResources {
        slack_message_search_port: Arc::new(OfflineSlack),
        slack_file_download_port: Arc::new(OfflineSlack),
        web_search_port: None,
        canvas_memory_port: None,
    }
}

struct OfflineSlack;

#[async_trait]
impl SlackMessageSearchPort for OfflineSlack {
    async fn search_messages(
        &self,
        _input: SlackMessageSearchInput,
    ) -> Result<SlackMessageSearchResult, PortError> {
        Err(PortError::invalid_input(
            "Slack is not available during evaluation",
        ))
    }
}

#[async_trait]
impl SlackFileDownloadPort for OfflineSlack {
    async fn download_file(&self, _url: &str, _max_bytes: u64) -> Result<Vec<u8>, PortError> {
        Err(PortError::invalid_input(
            "Slack is not available during evaluation",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{EvalOptions, run_eval};

    #[tokio::test]
    async fn replay_writes_json_and_markdown_and_compares_with_baseline() {
        let dir = std::env::temp_dir().join(format!("reili-eval-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let output = dir.join("eval.json");
        let markdown = dir.join("eval.md");

        let first = run_eval(EvalOptions {
            replay: true,
            output: Some(output.clone()),
            markdown: Some(markdown.clone()),
            ..EvalOptions::default()
        })
        .await
        .expect("replay");
        assert!(first.succeeded());
        assert!(
            std::fs::read_to_string(&markdown)
                .expect("markdown")
                .contains("**Result:** PASS")
        );

        let second = run_eval(EvalOptions {
            replay: true,
            baseline: Some(output.clone()),
            markdown: Some(markdown.clone()),
            ..EvalOptions::default()
        })
        .await
        .expect("replay against baseline");
        let comparison = second.comparison.expect("comparison");
        assert!(comparison.regressions.is_empty());
        assert!(comparison.accuracy_delta.abs() < f64::EPSILON);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
