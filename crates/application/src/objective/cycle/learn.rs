use reili_core::messaging::slack::{AppendSlackCanvasMemoryInput, SlackCanvasMemoryVisibility};
use reili_core::objective::ObjectiveDefinition;
use reili_core::work::KnowledgeCandidate;

use super::ObjectiveCycleRunner;
use super::observe::Observed;

const MAX_FACTS_PER_CYCLE: usize = 5;

impl ObjectiveCycleRunner {
    pub(super) async fn learn(
        &self,
        objective: &ObjectiveDefinition,
        run_id: &str,
        observed: &Observed,
        candidates: &[KnowledgeCandidate],
    ) {
        let Some(canvas) = self.deps.task_resources.canvas_memory_port.as_ref() else {
            return;
        };
        let mut known: Vec<String> = observed
            .memories
            .iter()
            .map(|memory| normalize(&memory.fact))
            .collect();
        for candidate in candidates.iter().take(MAX_FACTS_PER_CYCLE) {
            let fact = normalize(&candidate.fact);
            if known.contains(&fact) {
                continue;
            }
            let result = canvas
                .append_memory(AppendSlackCanvasMemoryInput {
                    visibility: if candidate.shared {
                        SlackCanvasMemoryVisibility::Shared
                    } else {
                        SlackCanvasMemoryVisibility::Channel
                    },
                    channel_id: objective.report_channel_id.clone(),
                    channel_name: None,
                    source_message_ts: String::new(),
                    fact: candidate.fact.clone(),
                    evidence: candidate.evidence.clone(),
                    scope: candidate.scope.clone(),
                })
                .await;
            match result {
                Ok(()) => known.push(fact),
                Err(error) => self.log(
                    "objective_cycle_learn_failed",
                    objective,
                    run_id,
                    Some(&error.message),
                ),
            }
        }
    }
}

fn normalize(fact: &str) -> String {
    fact.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
