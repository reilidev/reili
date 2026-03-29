use std::time::Duration;

use super::{SlackAnyChunk, SlackChunkSourceType, SlackTaskUpdateStatus};

pub(crate) const STREAM_ROTATION_CHARACTER_LIMIT: usize = 2900;
// Rotate before 300 seconds because Slack starts rejecting long-lived streams around that mark.
// This 300-second threshold is based on observed behavior, not an explicitly documented limit.
pub(crate) const STREAM_ROTATION_MAX_AGE: Duration = Duration::from_secs(270);

pub(crate) fn should_rotate_stream(
    current_stream_character_count: usize,
    current_stream_elapsed: Duration,
    chunks: &[SlackAnyChunk],
) -> bool {
    current_stream_elapsed >= STREAM_ROTATION_MAX_AGE
        || current_stream_character_count.saturating_add(count_chunk_characters(chunks))
            > STREAM_ROTATION_CHARACTER_LIMIT
}

pub(crate) fn count_chunk_characters(chunks: &[SlackAnyChunk]) -> usize {
    chunks.iter().map(count_single_chunk_characters).sum()
}

fn count_single_chunk_characters(chunk: &SlackAnyChunk) -> usize {
    match chunk {
        SlackAnyChunk::MarkdownText(chunk) => chunk.text.chars().count(),
        SlackAnyChunk::PlanUpdate(chunk) => chunk.title.chars().count(),
        SlackAnyChunk::TaskUpdate(chunk) => {
            let details_character_count = chunk
                .details
                .as_ref()
                .map_or(0, |details| details.chars().count());
            let output_character_count = chunk
                .output
                .as_ref()
                .map_or(0, |output| output.chars().count());
            let sources_character_count = chunk.sources.as_ref().map_or(0, |sources| {
                sources
                    .iter()
                    .map(|source| {
                        source.url.chars().count()
                            + source.text.chars().count()
                            + count_chunk_source_type_characters(&source.source_type)
                    })
                    .sum::<usize>()
            });

            chunk.id.chars().count()
                + chunk.title.chars().count()
                + count_task_status_characters(&chunk.status)
                + details_character_count
                + output_character_count
                + sources_character_count
        }
    }
}

fn count_task_status_characters(status: &SlackTaskUpdateStatus) -> usize {
    match status {
        SlackTaskUpdateStatus::Pending => "pending".chars().count(),
        SlackTaskUpdateStatus::InProgress => "in_progress".chars().count(),
        SlackTaskUpdateStatus::Complete => "complete".chars().count(),
        SlackTaskUpdateStatus::Error => "error".chars().count(),
    }
}

fn count_chunk_source_type_characters(source_type: &SlackChunkSourceType) -> usize {
    match source_type {
        SlackChunkSourceType::Url => "url".chars().count(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        STREAM_ROTATION_CHARACTER_LIMIT, STREAM_ROTATION_MAX_AGE, count_chunk_characters,
        should_rotate_stream,
    };
    use crate::outbound::slack::progress_stream::progress_models::{
        SlackChunkSource, SlackMarkdownTextChunk, SlackPlanUpdateChunk,
    };
    use crate::outbound::slack::progress_stream::{
        SlackAnyChunk, SlackChunkSourceType, SlackTaskUpdateChunk, SlackTaskUpdateStatus,
    };

    #[test]
    fn counts_characters_across_chunk_types_and_optional_fields() {
        let chunks = vec![
            SlackAnyChunk::MarkdownText(SlackMarkdownTextChunk {
                text: "abc".to_string(),
            }),
            SlackAnyChunk::PlanUpdate(SlackPlanUpdateChunk {
                title: "Plan".to_string(),
            }),
            SlackAnyChunk::TaskUpdate(SlackTaskUpdateChunk {
                id: "step-1".to_string(),
                title: "Collect".to_string(),
                status: SlackTaskUpdateStatus::InProgress,
                details: Some("run\n".to_string()),
                output: Some("done".to_string()),
                sources: Some(vec![SlackChunkSource {
                    source_type: SlackChunkSourceType::Url,
                    url: "https://a".to_string(),
                    text: "A".to_string(),
                }]),
            }),
        ];

        assert_eq!(count_chunk_characters(&chunks), 52);
    }

    #[test]
    fn does_not_rotate_when_total_reaches_exact_limit() {
        let chunks = vec![SlackAnyChunk::MarkdownText(SlackMarkdownTextChunk {
            text: "1234567890".to_string(),
        })];

        assert!(!should_rotate_stream(
            STREAM_ROTATION_CHARACTER_LIMIT - 10,
            Duration::from_secs(0),
            &chunks
        ));
    }

    #[test]
    fn rotates_when_total_exceeds_limit() {
        let chunks = vec![SlackAnyChunk::MarkdownText(SlackMarkdownTextChunk {
            text: "1234567890".to_string(),
        })];

        assert!(should_rotate_stream(
            STREAM_ROTATION_CHARACTER_LIMIT - 9,
            Duration::from_secs(0),
            &chunks
        ));
    }

    #[test]
    fn rotates_when_current_count_is_saturated() {
        let chunks = vec![SlackAnyChunk::MarkdownText(SlackMarkdownTextChunk {
            text: "a".to_string(),
        })];

        assert!(should_rotate_stream(
            usize::MAX,
            Duration::from_secs(0),
            &chunks
        ));
    }

    #[test]
    fn rotates_when_stream_age_reaches_limit() {
        let chunks = vec![SlackAnyChunk::MarkdownText(SlackMarkdownTextChunk {
            text: "a".to_string(),
        })];

        assert!(should_rotate_stream(0, STREAM_ROTATION_MAX_AGE, &chunks));
    }

    #[test]
    fn does_not_rotate_when_stream_age_is_below_limit_and_character_limit_not_exceeded() {
        let chunks = vec![SlackAnyChunk::MarkdownText(SlackMarkdownTextChunk {
            text: "a".to_string(),
        })];

        assert!(!should_rotate_stream(
            0,
            STREAM_ROTATION_MAX_AGE - Duration::from_secs(1),
            &chunks
        ));
    }
}
