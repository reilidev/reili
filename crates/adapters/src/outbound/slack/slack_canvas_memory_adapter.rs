//! Slack Canvas backed channel memory.
//!
//! A single shared Canvas (identified by `canvas_id`) holds every channel's memories. Each channel
//! is an H2 section keyed by `channel_id`; each memory is an H3 subsection whose heading is the
//! ISO 8601 UTC save time. All Canvas/markdown concerns are contained here so the rest of the
//! system only sees the semantic [`SlackCanvasMemoryPort`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    AppendSlackCanvasMemoryInput, ListSlackCanvasMemoriesInput, SlackCanvasMemoryPort,
    SlackCanvasMemoryRecord, SlackCanvasMemoryVisibility,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::slack_web_api_client::SlackWebApiClient;

/// Canvas markdown is small (a handful of channel sections); cap the download defensively.
const MAX_CANVAS_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_MEMORY_CAP: u32 = 15;
const CANVAS_EDITING_LOCKED: &str = "canvas_editing_locked";
const MAX_EDIT_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone)]
pub struct SlackCanvasMemoryAdapterConfig {
    pub canvas_id: String,
    /// Per-channel retention cap; falls back to [`DEFAULT_MEMORY_CAP`] when `None`.
    pub cap: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SlackCanvasMemoryAdapter {
    client: Arc<SlackWebApiClient>,
    canvas_id: String,
    cap: u32,
}

impl SlackCanvasMemoryAdapter {
    pub fn new(client: Arc<SlackWebApiClient>, config: SlackCanvasMemoryAdapterConfig) -> Self {
        Self {
            client,
            canvas_id: config.canvas_id,
            cap: config.cap.unwrap_or(DEFAULT_MEMORY_CAP).max(1),
        }
    }

    async fn fetch_canvas_markdown(&self) -> Result<String, PortError> {
        let response = self
            .client
            .get("files.info", &json!({ "file": self.canvas_id }))
            .await
            .map_err(explain_canvas_access_error)?;
        let parsed: FilesInfoResponse = serde_json::from_value(response).map_err(|error| {
            PortError::invalid_response(format!(
                "Failed to parse Slack files.info response for canvas: {error}"
            ))
        })?;

        if let Some(content) = parsed.file.inline_content() {
            return Ok(content);
        }

        let url = parsed.file.download_url().ok_or_else(|| {
            PortError::invalid_response(
                "Slack files.info response for canvas has no inline content and no download URL",
            )
        })?;
        // Slack serves canvas content only as an HTML rendering, so accept HTML and convert it back
        // to the markdown shape the parser expects.
        let bytes = self
            .client
            .download_bytes_allowing_html(url, MAX_CANVAS_BYTES)
            .await?;
        let body = String::from_utf8(bytes).map_err(|error| {
            PortError::invalid_response(format!("Canvas content is not valid UTF-8: {error}"))
        })?;
        Ok(html::to_markdown(&body))
    }

    async fn lookup_section_id(
        &self,
        section_type: &str,
        contains_text: &str,
    ) -> Result<Option<String>, PortError> {
        let response = self
            .client
            .post(
                "canvases.sections.lookup",
                &json!({
                    "canvas_id": self.canvas_id,
                    "criteria": {
                        "section_types": [section_type],
                        "contains_text": contains_text,
                    },
                }),
            )
            .await?;
        let parsed: SectionsLookupResponse = serde_json::from_value(response).map_err(|error| {
            PortError::invalid_response(format!(
                "Failed to parse Slack canvases.sections.lookup response: {error}"
            ))
        })?;

        Ok(parsed.sections.into_iter().next().map(|section| section.id))
    }

    async fn resolve_source_permalink(&self, channel_id: &str, message_ts: &str) -> Option<String> {
        let response = self
            .client
            .get(
                "chat.getPermalink",
                &json!({ "channel": channel_id, "message_ts": message_ts }),
            )
            .await;
        match response {
            Ok(value) => value
                .get("permalink")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|permalink| !permalink.is_empty())
                .map(ToString::to_string),
            Err(error) => {
                tracing::warn!(
                    channel_id = channel_id,
                    error = error.message,
                    "Failed to resolve Slack permalink for canvas memory source"
                );
                None
            }
        }
    }

    /// Best-effort channel display name for the section heading. Soft-fails to `None` so a lookup
    /// error just falls back to an id-only heading (`## C001`).
    async fn resolve_channel_name(&self, channel_id: &str) -> Option<String> {
        match self
            .client
            .get("conversations.info", &json!({ "channel": channel_id }))
            .await
        {
            Ok(value) => value
                .get("channel")
                .and_then(|channel| channel.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToString::to_string),
            Err(error) => {
                tracing::warn!(
                    channel_id = channel_id,
                    error = error.message,
                    "Failed to resolve Slack channel name for canvas memory section"
                );
                None
            }
        }
    }

    async fn apply_edit(&self, changes: Vec<Value>) -> Result<(), PortError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let result = self
                .client
                .post(
                    "canvases.edit",
                    &json!({ "canvas_id": self.canvas_id, "changes": changes }),
                )
                .await;
            match result {
                Ok(_) => return Ok(()),
                Err(error)
                    if error.is_service_error_code(CANVAS_EDITING_LOCKED)
                        && attempt < MAX_EDIT_ATTEMPTS =>
                {
                    tokio::time::sleep(edit_retry_backoff(attempt)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

#[async_trait]
impl SlackCanvasMemoryPort for SlackCanvasMemoryAdapter {
    async fn list_channel_memories(
        &self,
        input: ListSlackCanvasMemoriesInput,
    ) -> Result<Vec<SlackCanvasMemoryRecord>, PortError> {
        let markdown = self.fetch_canvas_markdown().await?;
        // Shared memories apply to every channel; keep them as their own capped group ahead of the
        // channel's own memories so they are never crowded out.
        let mut shared =
            markdown::parse_section_memories(&markdown, markdown::SectionMatch::Shared);
        shared.truncate(input.limit as usize);
        let mut channel = markdown::parse_section_memories(
            &markdown,
            markdown::SectionMatch::Channel(&input.channel_id),
        );
        channel.truncate(input.limit as usize);
        shared.extend(channel);
        Ok(shared)
    }

    async fn append_memory(&self, input: AppendSlackCanvasMemoryInput) -> Result<(), PortError> {
        let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        let source_url = self
            .resolve_source_permalink(&input.channel_id, &input.source_message_ts)
            .await;

        let section = match input.visibility {
            SlackCanvasMemoryVisibility::Channel => {
                markdown::SectionMatch::Channel(&input.channel_id)
            }
            SlackCanvasMemoryVisibility::Shared => markdown::SectionMatch::Shared,
        };
        let existing =
            markdown::parse_section_memories(&self.fetch_canvas_markdown().await?, section);

        let entry = markdown::MemoryEntry {
            created_at: created_at.clone(),
            fact: input.fact.trim().to_string(),
            evidence: input.evidence.trim().to_string(),
            scope: input.scope.trim().to_string(),
            source_url,
        };

        let mut changes: Vec<Value> = Vec::new();
        let section_id = self.lookup_section_id("h2", section.lookup_text()).await?;
        match section_id {
            Some(section_id) => changes.push(json!({
                "operation": "insert_after",
                "section_id": section_id,
                "document_content": markdown_document(&markdown::render_memory_entry(&entry)),
            })),
            None => {
                let new_section = self.render_new_section(&input, &entry).await;
                changes.push(json!({
                    "operation": "insert_at_end",
                    "document_content": markdown_document(&new_section),
                }));
            }
        }

        for timestamp in markdown::overflow_timestamps(&existing, self.cap) {
            if let Some(section_id) = self.lookup_section_id("h3", &timestamp).await? {
                changes.push(json!({
                    "operation": "delete",
                    "section_id": section_id,
                }));
            }
        }

        self.apply_edit(changes).await
    }
}

impl SlackCanvasMemoryAdapter {
    /// Renders a section heading plus its first entry when a section must be created. For a channel
    /// section the display name is resolved once here (an id-only heading is used on failure); the
    /// shared section has a fixed heading.
    async fn render_new_section(
        &self,
        input: &AppendSlackCanvasMemoryInput,
        entry: &markdown::MemoryEntry,
    ) -> String {
        match input.visibility {
            SlackCanvasMemoryVisibility::Channel => {
                let channel_name = match input.channel_name.clone() {
                    Some(name) => Some(name),
                    None => self.resolve_channel_name(&input.channel_id).await,
                };
                markdown::render_channel_section(&input.channel_id, channel_name.as_deref(), entry)
            }
            SlackCanvasMemoryVisibility::Shared => markdown::render_shared_section(entry),
        }
    }
}

fn markdown_document(markdown: &str) -> Value {
    json!({ "type": "markdown", "markdown": markdown })
}

/// `files.info` returns `not_visible`/`file_not_found` when the bot token cannot reach the canvas.
/// Canvases default to "only invited people can access", so a manually created canvas is invisible
/// to the bot until it is shared. Rewrite the opaque code into actionable guidance.
fn explain_canvas_access_error(error: PortError) -> PortError {
    match error.service_error_code() {
        Some(code @ ("not_visible" | "file_not_found")) => PortError::service_error(
            code,
            format!(
                "{}: the memory canvas is not accessible to the bot. Share the canvas with a \
channel the bot is a member of (with edit access), or grant the bot access via \
canvases.access.set, and confirm `canvas_id` points at that canvas.",
                error.message
            ),
        ),
        _ => error,
    }
}

/// Bounded backoff with time-derived jitter so concurrent editors do not retry in lockstep.
fn edit_retry_backoff(attempt: u32) -> Duration {
    let base = Duration::from_millis(50 * u64::from(attempt));
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| Duration::from_millis(u64::from(elapsed.subsec_millis()) % 50))
        .unwrap_or_default();
    base + jitter
}

#[derive(Debug, Deserialize)]
struct FilesInfoResponse {
    file: CanvasFileInfo,
}

#[derive(Debug, Deserialize)]
struct CanvasFileInfo {
    url_private: Option<String>,
    url_private_download: Option<String>,
    // Slack does not officially expose canvas markdown, but `files.info` may inline the full content
    // in one of these fields depending on file type. Preferred over the download path, whose
    // `url_private` serves an HTML rendering for canvases. `preview_plain_text` is deliberately not
    // used: it is truncated and would parse into partial records.
    contents: Option<String>,
    plain_text: Option<String>,
}

impl CanvasFileInfo {
    fn inline_content(&self) -> Option<String> {
        [&self.contents, &self.plain_text]
            .into_iter()
            .flatten()
            .map(|value| value.trim())
            .find(|value| !value.is_empty())
            .map(ToString::to_string)
    }

    fn download_url(&self) -> Option<&str> {
        self.url_private_download
            .as_deref()
            .or(self.url_private.as_deref())
            .map(str::trim)
            .filter(|url| !url.is_empty())
    }
}

#[derive(Debug, Deserialize)]
struct SectionsLookupResponse {
    #[serde(default)]
    sections: Vec<CanvasSection>,
}

#[derive(Debug, Deserialize)]
struct CanvasSection {
    id: String,
}

/// Pure markdown layout: rendering channel / shared sections and memory entries, and parsing them
/// back.
mod markdown {
    use reili_core::messaging::slack::{SlackCanvasMemoryRecord, SlackCanvasMemoryVisibility};

    const FACT_LABEL: &str = "**Fact**:";
    const EVIDENCE_LABEL: &str = "**Evidence**:";
    const SCOPE_LABEL: &str = "**Scope**:";
    const SOURCE_LABEL: &str = "**Source**:";

    /// H2 heading text (after `## `) of the reserved section holding memories shared across every
    /// channel. Chosen to never collide with a channel heading (`## #name · Cxxx`).
    pub(super) const SHARED_SECTION_TITLE: &str = "Shared memory (all channels)";

    /// Selects which H2 section to read or write.
    #[derive(Debug, Clone, Copy)]
    pub(super) enum SectionMatch<'a> {
        Channel(&'a str),
        Shared,
    }

    impl SectionMatch<'_> {
        fn visibility(&self) -> SlackCanvasMemoryVisibility {
            match self {
                Self::Channel(_) => SlackCanvasMemoryVisibility::Channel,
                Self::Shared => SlackCanvasMemoryVisibility::Shared,
            }
        }

        fn matches(&self, heading: &str) -> bool {
            match self {
                Self::Channel(channel_id) => heading_matches_channel(heading, channel_id),
                Self::Shared => heading.trim() == SHARED_SECTION_TITLE,
            }
        }

        /// The `contains_text` value used to locate this section via `canvases.sections.lookup`.
        pub(super) fn lookup_text(&self) -> &str {
            match self {
                Self::Channel(channel_id) => channel_id,
                Self::Shared => SHARED_SECTION_TITLE,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct MemoryEntry {
        pub(super) created_at: String,
        pub(super) fact: String,
        pub(super) evidence: String,
        pub(super) scope: String,
        pub(super) source_url: Option<String>,
    }

    /// Heading for a channel's H2 section. The `channel_id` is the stable lookup key; the display
    /// name is appended for human readers when known.
    pub(super) fn channel_heading(channel_id: &str, channel_name: Option<&str>) -> String {
        match channel_name.map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => format!("## #{name} · {channel_id}"),
            None => format!("## {channel_id}"),
        }
    }

    pub(super) fn shared_heading() -> String {
        format!("## {SHARED_SECTION_TITLE}")
    }

    pub(super) fn render_shared_section(entry: &MemoryEntry) -> String {
        format!("{}\n{}", shared_heading(), render_memory_entry(entry))
    }

    pub(super) fn render_memory_entry(entry: &MemoryEntry) -> String {
        let mut blocks = vec![
            format!("### {}", entry.created_at),
            format!("{FACT_LABEL} {}", entry.fact),
            format!("{EVIDENCE_LABEL} {}", entry.evidence),
            format!("{SCOPE_LABEL} {}", entry.scope),
        ];
        if let Some(source_url) = entry
            .source_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
        {
            blocks.push(format!("{SOURCE_LABEL} {source_url}"));
        }
        // Separate each field with a blank line so markdown renders them as distinct paragraphs.
        // A single newline is a soft break that Slack collapses into a space, joining every field
        // onto one line.
        format!("{}\n", blocks.join("\n\n"))
    }

    pub(super) fn render_channel_section(
        channel_id: &str,
        channel_name: Option<&str>,
        entry: &MemoryEntry,
    ) -> String {
        format!(
            "{}\n{}",
            channel_heading(channel_id, channel_name),
            render_memory_entry(entry)
        )
    }

    /// Returns the timestamps of the oldest entries that must be deleted so that, after one new
    /// entry is inserted, the channel keeps at most `cap` memories. `existing` need not be sorted.
    pub(super) fn overflow_timestamps(
        existing: &[SlackCanvasMemoryRecord],
        cap: u32,
    ) -> Vec<String> {
        let keep_existing = (cap as usize).saturating_sub(1);
        if existing.len() <= keep_existing {
            return Vec::new();
        }
        let mut sorted: Vec<&SlackCanvasMemoryRecord> = existing.iter().collect();
        // Newest first; the tail beyond `keep_existing` is the overflow to prune.
        sorted.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        sorted
            .into_iter()
            .skip(keep_existing)
            .map(|record| record.created_at.clone())
            .collect()
    }

    /// Parses the H3 memory entries under the H2 section selected by `section`, tagged with that
    /// section's visibility and returned newest-first by heading timestamp.
    pub(super) fn parse_section_memories(
        markdown: &str,
        section: SectionMatch<'_>,
    ) -> Vec<SlackCanvasMemoryRecord> {
        let visibility = section.visibility();
        let mut records: Vec<SlackCanvasMemoryRecord> = Vec::new();
        let mut in_target_section = false;
        let mut current: Option<PartialEntry> = None;

        for line in markdown.lines() {
            if let Some(heading) = line.strip_prefix("### ") {
                flush(&mut current, &mut records, in_target_section, visibility);
                if in_target_section {
                    current = Some(PartialEntry::new(heading.trim()));
                }
                continue;
            }
            if let Some(heading) = strip_h2(line) {
                flush(&mut current, &mut records, in_target_section, visibility);
                in_target_section = section.matches(heading);
                continue;
            }
            if let Some(entry) = current.as_mut()
                && in_target_section
            {
                entry.absorb(line);
            }
        }
        flush(&mut current, &mut records, in_target_section, visibility);

        records.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        records
    }

    fn strip_h2(line: &str) -> Option<&str> {
        // H3 lines (`### `) are consumed by the caller before this runs, so a leading `## ` here is
        // unambiguously an H2 even when the heading text itself begins with `#` (e.g. `## #alerts`).
        line.strip_prefix("## ")
    }

    fn heading_matches_channel(heading: &str, channel_id: &str) -> bool {
        heading
            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
            .any(|token| token == channel_id)
    }

    /// Matches a `Fact`/`Evidence`/`Scope`/`Source` label followed by a colon, tolerating the bold
    /// variants we render (`**Fact**:`), the older `**Fact:**`, and a de-emphasized `Fact:` from an
    /// HTML rendering. `label` is the word without a colon.
    fn strip_label<'a>(line: &'a str, label: &str) -> Option<&'a str> {
        let rest = line.trim_start_matches('*');
        let rest = rest.strip_prefix(label)?;
        let rest = rest.trim_start_matches('*').strip_prefix(':')?;
        Some(rest.trim_start_matches('*').trim())
    }

    fn flush(
        current: &mut Option<PartialEntry>,
        records: &mut Vec<SlackCanvasMemoryRecord>,
        in_target_section: bool,
        visibility: SlackCanvasMemoryVisibility,
    ) {
        if !in_target_section {
            *current = None;
            return;
        }
        if let Some(entry) = current.take()
            && let Some(record) = entry.into_record(visibility)
        {
            records.push(record);
        }
    }

    struct PartialEntry {
        created_at: String,
        fact: Option<String>,
        evidence: Option<String>,
        scope: Option<String>,
        source_url: Option<String>,
    }

    impl PartialEntry {
        fn new(created_at: &str) -> Self {
            Self {
                created_at: created_at.to_string(),
                fact: None,
                evidence: None,
                scope: None,
                source_url: None,
            }
        }

        fn absorb(&mut self, line: &str) {
            let trimmed = line.trim();
            if let Some(value) = strip_label(trimmed, "Fact") {
                self.fact = Some(value.to_string());
            } else if let Some(value) = strip_label(trimmed, "Evidence") {
                self.evidence = Some(value.to_string());
            } else if let Some(value) = strip_label(trimmed, "Scope") {
                self.scope = Some(value.to_string());
            } else if let Some(value) = strip_label(trimmed, "Source")
                && !value.is_empty()
            {
                self.source_url = Some(value.to_string());
            }
        }

        fn into_record(
            self,
            visibility: SlackCanvasMemoryVisibility,
        ) -> Option<SlackCanvasMemoryRecord> {
            if self.created_at.is_empty() {
                return None;
            }
            Some(SlackCanvasMemoryRecord {
                visibility,
                fact: self.fact.unwrap_or_default(),
                evidence: self.evidence.unwrap_or_default(),
                scope: self.scope.unwrap_or_default(),
                source_url: self.source_url,
                created_at: self.created_at,
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use reili_core::messaging::slack::{SlackCanvasMemoryRecord, SlackCanvasMemoryVisibility};

        use super::{
            MemoryEntry, SectionMatch, channel_heading, overflow_timestamps,
            parse_section_memories, render_channel_section, render_memory_entry,
            render_shared_section,
        };

        fn parse_channel_memories(
            markdown: &str,
            channel_id: &str,
        ) -> Vec<SlackCanvasMemoryRecord> {
            parse_section_memories(markdown, SectionMatch::Channel(channel_id))
        }

        fn record(created_at: &str) -> SlackCanvasMemoryRecord {
            SlackCanvasMemoryRecord {
                visibility: SlackCanvasMemoryVisibility::Channel,
                fact: "f".to_string(),
                evidence: "e".to_string(),
                scope: "s".to_string(),
                source_url: None,
                created_at: created_at.to_string(),
            }
        }

        fn sample_entry() -> MemoryEntry {
            MemoryEntry {
                created_at: "2026-07-07T09:12:34Z".to_string(),
                fact: "checkout-api owns the /checkout route".to_string(),
                evidence: "confirmed in services/checkout README".to_string(),
                scope: "checkout production".to_string(),
                source_url: Some("https://slack/permalink".to_string()),
            }
        }

        #[test]
        fn channel_heading_appends_display_name_when_present() {
            assert_eq!(channel_heading("C001", Some("alerts")), "## #alerts · C001");
            assert_eq!(channel_heading("C001", None), "## C001");
            assert_eq!(channel_heading("C001", Some("  ")), "## C001");
        }

        #[test]
        fn render_and_parse_round_trip() {
            let markdown = render_channel_section("C001", Some("alerts"), &sample_entry());

            let records = parse_channel_memories(&markdown, "C001");

            assert_eq!(
                records,
                vec![SlackCanvasMemoryRecord {
                    visibility: SlackCanvasMemoryVisibility::Channel,
                    fact: "checkout-api owns the /checkout route".to_string(),
                    evidence: "confirmed in services/checkout README".to_string(),
                    scope: "checkout production".to_string(),
                    source_url: Some("https://slack/permalink".to_string()),
                    created_at: "2026-07-07T09:12:34Z".to_string(),
                }]
            );
        }

        #[test]
        fn shared_section_round_trip_tags_visibility_and_ignores_channel_sections() {
            let markdown = format!(
                "{}\n{}## #alerts · C001\n### 2026-07-07T00:00:00Z\n**Fact:** channel only\n",
                render_shared_section(&MemoryEntry {
                    created_at: "2026-07-08T10:00:00Z".to_string(),
                    fact: "the org standard CI is GitHub Actions".to_string(),
                    evidence: "engineering handbook".to_string(),
                    scope: "all repositories".to_string(),
                    source_url: Some("https://slack/handbook".to_string()),
                }),
                "\n",
            );

            let shared = parse_section_memories(&markdown, SectionMatch::Shared);

            assert_eq!(shared.len(), 1);
            assert_eq!(shared[0].visibility, SlackCanvasMemoryVisibility::Shared);
            assert_eq!(shared[0].fact, "the org standard CI is GitHub Actions");
            // The channel section is not part of the shared section.
            assert!(!shared.iter().any(|r| r.fact == "channel only"));
        }

        #[test]
        fn render_memory_entry_omits_source_line_when_absent() {
            let entry = MemoryEntry {
                source_url: None,
                ..sample_entry()
            };

            let rendered = render_memory_entry(&entry);

            assert!(!rendered.contains("Source:"));
            assert!(rendered.starts_with("### 2026-07-07T09:12:34Z"));
        }

        #[test]
        fn render_memory_entry_separates_fields_with_blank_lines() {
            let rendered = render_memory_entry(&sample_entry());

            // Blank lines make markdown render each field as its own paragraph rather than
            // collapsing them onto a single line via soft breaks.
            assert!(rendered.contains(
                "**Fact**: checkout-api owns the /checkout route\n\n**Evidence**: confirmed in services/checkout README"
            ));
            assert!(
                rendered.contains(
                    "**Scope**: checkout production\n\n**Source**: https://slack/permalink"
                )
            );
        }

        #[test]
        fn parse_selects_only_the_matching_channel_section_newest_first() {
            let markdown = "\
## #alerts · C001
### 2026-07-06T22:40:11Z
**Fact:** older fact
**Evidence:** older evidence
**Scope:** older scope
Source: https://slack/older
### 2026-07-07T09:12:34Z
**Fact:** newer fact
**Evidence:** newer evidence
**Scope:** newer scope
Source: https://slack/newer

## #incidents · C777
### 2026-07-05T14:03:58Z
**Fact:** other channel fact
**Evidence:** other evidence
**Scope:** other scope
";

            let records = parse_channel_memories(markdown, "C001");

            assert_eq!(records.len(), 2);
            assert_eq!(records[0].created_at, "2026-07-07T09:12:34Z");
            assert_eq!(records[0].fact, "newer fact");
            assert_eq!(records[1].created_at, "2026-07-06T22:40:11Z");
        }

        #[test]
        fn parse_returns_empty_when_channel_section_absent() {
            let markdown = "## #incidents · C777\n### 2026-07-05T14:03:58Z\n**Fact:** x\n";

            assert!(parse_channel_memories(markdown, "C001").is_empty());
        }

        #[test]
        fn overflow_timestamps_prunes_oldest_beyond_cap_after_insert() {
            let existing = vec![
                record("2026-07-07T00:00:03Z"),
                record("2026-07-07T00:00:01Z"),
                record("2026-07-07T00:00:02Z"),
            ];

            // cap 3, inserting one more -> keep 2 newest existing, prune the oldest.
            let overflow = overflow_timestamps(&existing, 3);

            assert_eq!(overflow, vec!["2026-07-07T00:00:01Z".to_string()]);
        }

        #[test]
        fn overflow_timestamps_empty_when_under_cap() {
            let existing = vec![record("2026-07-07T00:00:01Z")];

            assert!(overflow_timestamps(&existing, 10).is_empty());
        }
    }
}

/// Converts the HTML rendering Slack serves for a canvas back into the markdown shape
/// [`markdown::parse_section_memories`] expects. Headings become `##`/`###` lines, bold spans are
/// restored to `**...**`, and block/line-break tags become newlines; other tags are dropped and
/// entities decoded. This mirrors the exact markdown we write in [`markdown::render_memory_entry`].
mod html {
    pub(super) fn to_markdown(html: &str) -> String {
        let mut out = String::new();
        let mut rest = html;

        while let Some(open) = rest.find('<') {
            append_text(&mut out, &rest[..open]);
            let Some(close) = rest[open..].find('>') else {
                // Unterminated tag; treat the remainder as text.
                append_text(&mut out, &rest[open..]);
                rest = "";
                break;
            };
            let tag = &rest[open + 1..open + close];
            apply_tag(&mut out, tag);
            rest = &rest[open + close + 1..];
        }
        append_text(&mut out, rest);

        normalize_newlines(&out)
    }

    fn apply_tag(out: &mut String, raw_tag: &str) {
        let tag = raw_tag.trim();
        let (closing, name_part) = match tag.strip_prefix('/') {
            Some(name) => (true, name),
            None => (false, tag),
        };
        let name = name_part
            .split([' ', '\t', '\n', '/', '>'])
            .next()
            .unwrap_or("")
            .trim_end_matches('/')
            .to_ascii_lowercase();

        match name.as_str() {
            "h1" if !closing => push_prefix(out, "# "),
            "h2" if !closing => push_prefix(out, "## "),
            "h3" | "h4" | "h5" | "h6" if !closing => push_prefix(out, "### "),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => out.push('\n'),
            "strong" | "b" => out.push_str("**"),
            "em" | "i" => out.push('*'),
            "br" => out.push('\n'),
            "p" | "div" | "li" | "ul" | "ol" | "tr" | "blockquote" | "section" if closing => {
                out.push('\n')
            }
            _ => {}
        }
    }

    fn push_prefix(out: &mut String, prefix: &str) {
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out.push_str(prefix);
    }

    fn append_text(out: &mut String, raw: &str) {
        if raw.is_empty() {
            return;
        }
        out.push_str(&decode_entities(raw));
    }

    fn decode_entities(input: &str) -> String {
        if !input.contains('&') {
            return input.to_string();
        }
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(amp) = rest.find('&') {
            out.push_str(&rest[..amp]);
            let after = &rest[amp..];
            match after.find(';').filter(|end| *end <= 10) {
                Some(end) => {
                    let entity = &after[1..end];
                    out.push_str(&decode_entity(entity));
                    rest = &after[end + 1..];
                }
                None => {
                    out.push('&');
                    rest = &after[1..];
                }
            }
        }
        out.push_str(rest);
        out
    }

    fn decode_entity(entity: &str) -> String {
        match entity {
            "amp" => "&".to_string(),
            "lt" => "<".to_string(),
            "gt" => ">".to_string(),
            "quot" => "\"".to_string(),
            "apos" | "#39" => "'".to_string(),
            "nbsp" | "#160" => " ".to_string(),
            "middot" | "#183" => "·".to_string(),
            other => other
                .strip_prefix('#')
                .and_then(|code| code.parse::<u32>().ok())
                .and_then(char::from_u32)
                .map(|c| c.to_string())
                .unwrap_or_else(|| format!("&{other};")),
        }
    }

    fn normalize_newlines(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut newline_run = 0;
        for ch in input.chars() {
            if ch == '\n' {
                newline_run += 1;
                if newline_run <= 2 {
                    out.push('\n');
                }
            } else {
                newline_run = 0;
                out.push(ch);
            }
        }
        out.trim().to_string()
    }

    #[cfg(test)]
    mod tests {
        use super::to_markdown;

        #[test]
        fn restores_headings_and_bold_labels_from_canvas_html() {
            let html = "<h2>#alerts · C001</h2>\
<h3>2026-07-07T09:12:34Z</h3>\
<p><strong>Fact:</strong> checkout-api owns /checkout</p>\
<p><strong>Evidence:</strong> services/checkout README</p>\
<p><strong>Scope:</strong> checkout production</p>\
<p>Source: https://slack/permalink</p>";

            let markdown = to_markdown(html);

            assert!(markdown.contains("## #alerts · C001"));
            assert!(markdown.contains("### 2026-07-07T09:12:34Z"));
            assert!(markdown.contains("**Fact:** checkout-api owns /checkout"));
            assert!(markdown.contains("**Scope:** checkout production"));
            assert!(markdown.contains("Source: https://slack/permalink"));
        }

        #[test]
        fn decodes_html_entities() {
            let html = "<p><strong>Fact:</strong> a &amp; b &lt; c &#39;d&#39;</p>";

            let markdown = to_markdown(html);

            assert!(markdown.contains("**Fact:** a & b < c 'd'"));
        }

        #[test]
        fn parsed_round_trip_from_rendered_html() {
            let html = "<h2>#alerts · C001</h2>\
<h3>2026-07-07T09:12:34Z</h3>\
<p><strong>Fact:</strong> f</p><p><strong>Evidence:</strong> e</p>\
<p><strong>Scope:</strong> s</p><p>Source: https://slack/x</p>";

            let records = super::super::markdown::parse_section_memories(
                &to_markdown(html),
                super::super::markdown::SectionMatch::Channel("C001"),
            );

            assert_eq!(records.len(), 1);
            assert_eq!(records[0].fact, "f");
            assert_eq!(records[0].source_url.as_deref(), Some("https://slack/x"));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::messaging::slack::{
        AppendSlackCanvasMemoryInput, ListSlackCanvasMemoriesInput, SlackCanvasMemoryPort,
        SlackCanvasMemoryVisibility,
    };
    use reili_core::secret::SecretString;
    use serde_json::{Value, json};
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::{
        SlackCanvasMemoryAdapter, SlackCanvasMemoryAdapterConfig, explain_canvas_access_error,
    };
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[test]
    fn explains_not_visible_canvas_access_error_with_guidance() {
        let error = reili_core::error::PortError::service_error(
            "not_visible",
            "Slack API returned error: method=files.info error=not_visible",
        );

        let explained = explain_canvas_access_error(error);

        assert_eq!(explained.service_error_code(), Some("not_visible"));
        assert!(explained.message.contains("not accessible to the bot"));
        assert!(explained.message.contains("canvases.access.set"));
    }

    fn create_adapter(base_url: &str, cap: Option<u32>) -> SlackCanvasMemoryAdapter {
        let client = SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: SecretString::from("xoxb-test"),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client");
        SlackCanvasMemoryAdapter::new(
            Arc::new(client),
            SlackCanvasMemoryAdapterConfig {
                canvas_id: "F0CANVAS".to_string(),
                cap,
            },
        )
    }

    async fn mount_files_info(server: &MockServer, markdown: &str) {
        let download_url = format!("{}/canvas/content", server.uri());
        Mock::given(method("GET"))
            .and(path("/files.info"))
            .and(query_param("file", "F0CANVAS"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file": { "url_private": download_url }
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/canvas/content"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/markdown")
                    .set_body_string(markdown.to_string()),
            )
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn lists_channel_memories_newest_first_capped_at_limit() {
        let server = MockServer::start().await;
        let markdown = "\
## #alerts · C001
### 2026-07-06T22:40:11Z
**Fact:** older fact
**Evidence:** older evidence
**Scope:** older scope
### 2026-07-07T09:12:34Z
**Fact:** newer fact
**Evidence:** newer evidence
**Scope:** newer scope
Source: https://slack/newer
";
        mount_files_info(&server, markdown).await;

        let adapter = create_adapter(&server.uri(), None);
        let records = adapter
            .list_channel_memories(ListSlackCanvasMemoriesInput {
                channel_id: "C001".to_string(),
                channel_name: Some("alerts".to_string()),
                limit: 1,
            })
            .await
            .expect("list channel memories");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].created_at, "2026-07-07T09:12:34Z");
        assert_eq!(records[0].fact, "newer fact");
        assert_eq!(
            records[0].source_url.as_deref(),
            Some("https://slack/newer")
        );
    }

    #[tokio::test]
    async fn prefers_inline_files_info_content_over_html_download() {
        let server = MockServer::start().await;
        // files.info inlines the canvas markdown; no download endpoint is mounted, so any attempt
        // to download would fail the test.
        Mock::given(method("GET"))
            .and(path("/files.info"))
            .and(query_param("file", "F0CANVAS"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file": {
                    "url_private": format!("{}/canvas/content", server.uri()),
                    "contents": "## #alerts · C001\n### 2026-07-07T09:12:34Z\n**Fact:** inline fact\n**Evidence:** e\n**Scope:** s\n"
                }
            })))
            .mount(&server)
            .await;

        let adapter = create_adapter(&server.uri(), None);
        let records = adapter
            .list_channel_memories(ListSlackCanvasMemoriesInput {
                channel_id: "C001".to_string(),
                channel_name: Some("alerts".to_string()),
                limit: 10,
            })
            .await
            .expect("list channel memories");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].fact, "inline fact");
    }

    #[tokio::test]
    async fn append_inserts_after_existing_channel_section() {
        let server = MockServer::start().await;
        mount_files_info(
            &server,
            "## #alerts · C001\n### 2026-07-06T22:40:11Z\n**Fact:** old\n**Evidence:** e\n**Scope:** s\n",
        )
        .await;

        Mock::given(method("GET"))
            .and(path("/chat.getPermalink"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "permalink": "https://slack/thread"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/canvases.sections.lookup"))
            .and(body_partial_json(json!({
                "canvas_id": "F0CANVAS",
                "criteria": { "section_types": ["h2"], "contains_text": "C001" }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "sections": [{ "id": "sec-h2-c001" }]
            })))
            .mount(&server)
            .await;

        let edit_body: Arc<std::sync::Mutex<Option<Value>>> = Arc::new(std::sync::Mutex::new(None));
        let captured = Arc::clone(&edit_body);
        Mock::given(method("POST"))
            .and(path("/canvases.edit"))
            .respond_with(move |request: &Request| {
                *captured.lock().expect("lock edit body") =
                    Some(request.body_json::<Value>().expect("edit body json"));
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true }))
            })
            .mount(&server)
            .await;

        let adapter = create_adapter(&server.uri(), Some(10));
        adapter
            .append_memory(AppendSlackCanvasMemoryInput {
                visibility: SlackCanvasMemoryVisibility::Channel,
                channel_id: "C001".to_string(),
                channel_name: Some("alerts".to_string()),
                source_message_ts: "1760000000.000001".to_string(),
                fact: "new fact".to_string(),
                evidence: "new evidence".to_string(),
                scope: "new scope".to_string(),
            })
            .await
            .expect("append memory");

        let body = edit_body
            .lock()
            .expect("lock edit body")
            .clone()
            .expect("edit body");
        let changes = body["changes"].as_array().expect("changes array");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0]["operation"], "insert_after");
        assert_eq!(changes[0]["section_id"], "sec-h2-c001");
        let markdown = changes[0]["document_content"]["markdown"]
            .as_str()
            .expect("markdown");
        assert!(markdown.contains("**Fact**: new fact"));
        assert!(markdown.contains("**Source**: https://slack/thread"));
    }

    #[tokio::test]
    async fn append_creates_channel_section_when_missing() {
        let server = MockServer::start().await;
        mount_files_info(
            &server,
            "## #incidents · C777\n### 2026-07-05T14:03:58Z\n**Fact:** x\n",
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/chat.getPermalink"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "permalink": "https://slack/thread"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/canvases.sections.lookup"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "sections": []
            })))
            .mount(&server)
            .await;
        let edit_body: Arc<std::sync::Mutex<Option<Value>>> = Arc::new(std::sync::Mutex::new(None));
        let captured = Arc::clone(&edit_body);
        Mock::given(method("POST"))
            .and(path("/canvases.edit"))
            .respond_with(move |request: &Request| {
                *captured.lock().expect("lock edit body") =
                    Some(request.body_json::<Value>().expect("edit body json"));
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true }))
            })
            .mount(&server)
            .await;

        let adapter = create_adapter(&server.uri(), Some(10));
        adapter
            .append_memory(AppendSlackCanvasMemoryInput {
                visibility: SlackCanvasMemoryVisibility::Channel,
                channel_id: "C001".to_string(),
                channel_name: Some("alerts".to_string()),
                source_message_ts: "1760000000.000001".to_string(),
                fact: "new fact".to_string(),
                evidence: "new evidence".to_string(),
                scope: "new scope".to_string(),
            })
            .await
            .expect("append memory");

        let body = edit_body
            .lock()
            .expect("lock edit body")
            .clone()
            .expect("edit body");
        let changes = body["changes"].as_array().expect("changes array");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0]["operation"], "insert_at_end");
        let markdown = changes[0]["document_content"]["markdown"]
            .as_str()
            .expect("markdown");
        assert!(markdown.contains("## #alerts · C001"));
        assert!(markdown.contains("**Fact**: new fact"));
    }

    #[tokio::test]
    async fn append_resolves_channel_display_name_when_creating_section() {
        let server = MockServer::start().await;
        mount_files_info(
            &server,
            "## #incidents · C777\n### 2026-07-05T14:03:58Z\n**Fact:** x\n",
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/chat.getPermalink"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "permalink": "https://slack/thread"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/conversations.info"))
            .and(query_param("channel", "C001"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": { "id": "C001", "name": "alerts" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/canvases.sections.lookup"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "sections": []
            })))
            .mount(&server)
            .await;
        let edit_body: Arc<std::sync::Mutex<Option<Value>>> = Arc::new(std::sync::Mutex::new(None));
        let captured = Arc::clone(&edit_body);
        Mock::given(method("POST"))
            .and(path("/canvases.edit"))
            .respond_with(move |request: &Request| {
                *captured.lock().expect("lock edit body") =
                    Some(request.body_json::<Value>().expect("edit body json"));
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true }))
            })
            .mount(&server)
            .await;

        let adapter = create_adapter(&server.uri(), Some(10));
        adapter
            .append_memory(AppendSlackCanvasMemoryInput {
                visibility: SlackCanvasMemoryVisibility::Channel,
                channel_id: "C001".to_string(),
                // Production passes no name; the adapter resolves it via conversations.info.
                channel_name: None,
                source_message_ts: "1760000000.000001".to_string(),
                fact: "new fact".to_string(),
                evidence: "new evidence".to_string(),
                scope: "new scope".to_string(),
            })
            .await
            .expect("append memory");

        let body = edit_body
            .lock()
            .expect("lock edit body")
            .clone()
            .expect("edit body");
        let markdown = body["changes"][0]["document_content"]["markdown"]
            .as_str()
            .expect("markdown");
        assert!(markdown.contains("## #alerts · C001"));
    }

    #[tokio::test]
    async fn append_falls_back_to_id_only_heading_when_name_lookup_fails() {
        let server = MockServer::start().await;
        mount_files_info(
            &server,
            "## #incidents · C777\n### 2026-07-05T14:03:58Z\n**Fact:** x\n",
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/chat.getPermalink"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "permalink": "https://slack/thread"
            })))
            .mount(&server)
            .await;
        // conversations.info fails; the section heading must still be written with the id.
        Mock::given(method("GET"))
            .and(path("/conversations.info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "channel_not_found"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/canvases.sections.lookup"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "sections": [] })),
            )
            .mount(&server)
            .await;
        let edit_body: Arc<std::sync::Mutex<Option<Value>>> = Arc::new(std::sync::Mutex::new(None));
        let captured = Arc::clone(&edit_body);
        Mock::given(method("POST"))
            .and(path("/canvases.edit"))
            .respond_with(move |request: &Request| {
                *captured.lock().expect("lock edit body") =
                    Some(request.body_json::<Value>().expect("edit body json"));
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true }))
            })
            .mount(&server)
            .await;

        let adapter = create_adapter(&server.uri(), Some(10));
        adapter
            .append_memory(AppendSlackCanvasMemoryInput {
                visibility: SlackCanvasMemoryVisibility::Channel,
                channel_id: "C001".to_string(),
                channel_name: None,
                source_message_ts: "1760000000.000001".to_string(),
                fact: "new fact".to_string(),
                evidence: "new evidence".to_string(),
                scope: "new scope".to_string(),
            })
            .await
            .expect("append memory");

        let body = edit_body
            .lock()
            .expect("lock edit body")
            .clone()
            .expect("edit body");
        let markdown = body["changes"][0]["document_content"]["markdown"]
            .as_str()
            .expect("markdown");
        assert!(markdown.contains("## C001"));
        assert!(!markdown.contains("#alerts"));
    }

    #[tokio::test]
    async fn append_deletes_overflow_entries_beyond_cap() {
        let server = MockServer::start().await;
        mount_files_info(
            &server,
            "## #alerts · C001\n### 2026-07-07T00:00:02Z\n**Fact:** b\n### 2026-07-07T00:00:01Z\n**Fact:** a\n",
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/chat.getPermalink"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": true, "permalink": "https://slack/thread" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/canvases.sections.lookup"))
            .and(body_partial_json(
                json!({ "criteria": { "section_types": ["h2"] } }),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": true, "sections": [{ "id": "sec-h2" }] })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/canvases.sections.lookup"))
            .and(body_partial_json(json!({ "criteria": { "section_types": ["h3"], "contains_text": "2026-07-07T00:00:01Z" } })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "sections": [{ "id": "sec-h3-oldest" }] })))
            .mount(&server)
            .await;
        let edit_body: Arc<std::sync::Mutex<Option<Value>>> = Arc::new(std::sync::Mutex::new(None));
        let captured = Arc::clone(&edit_body);
        Mock::given(method("POST"))
            .and(path("/canvases.edit"))
            .respond_with(move |request: &Request| {
                *captured.lock().expect("lock edit body") =
                    Some(request.body_json::<Value>().expect("edit body json"));
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true }))
            })
            .mount(&server)
            .await;

        // cap 2: after inserting one, only 1 existing may remain, so the oldest is pruned.
        let adapter = create_adapter(&server.uri(), Some(2));
        adapter
            .append_memory(AppendSlackCanvasMemoryInput {
                visibility: SlackCanvasMemoryVisibility::Channel,
                channel_id: "C001".to_string(),
                channel_name: Some("alerts".to_string()),
                source_message_ts: "1760000000.000001".to_string(),
                fact: "new".to_string(),
                evidence: "new".to_string(),
                scope: "new".to_string(),
            })
            .await
            .expect("append memory");

        let body = edit_body
            .lock()
            .expect("lock edit body")
            .clone()
            .expect("edit body");
        let changes = body["changes"].as_array().expect("changes array");
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[1]["operation"], "delete");
        assert_eq!(changes[1]["section_id"], "sec-h3-oldest");
    }
}
