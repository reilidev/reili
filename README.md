<div align="center">
  <h1>Reili</h1>
  <img src="./reili.svg" alt="Reili logo" width="240" />
  <p><strong>A Slack-native AI agent for read-only DevOps investigations</strong></p>
  <p>
    Investigate alerts quickly across Datadog, GitHub, and Slack threads.
    <br />
    Operate with a simple database-free architecture.
  </p>
</div>

## Why Reili

`Reili` starts from Slack messages and investigation requests, then:

- Investigates Datadog Logs, Metrics, and Events
- Explores GitHub repositories, PRs, Issues, and code

It focuses on read-oriented DevOps work: triage, investigation.

## Core Features

- Slack-native intake via `app_mention` events

### Runtime Characteristics

- Database-free: no persistent state component
- Job queue is in-memory, so pending jobs are lost on app restart

## Quick Start

### 1. Prerequisites

- Rust stable toolchain
- Slack App (Bot Token / Signing Secret)
- Datadog API Key + APP Key
- OpenAI API Key
- GitHub App (App ID / Private Key / Installation ID)

### 2. Install

```bash
cp .env.example .env
```

### 3. Configure Environment Variables

Required:

- `SLACK_BOT_TOKEN`
- `SLACK_SIGNING_SECRET`
- `DATADOG_API_KEY`
- `DATADOG_APP_KEY`
- `LLM_PROVIDER`
- `LLM_OPENAI_API_KEY` when `LLM_PROVIDER=openai`
- `LLM_BEDROCK_REGION` when `LLM_PROVIDER=bedrock`
- `LLM_BEDROCK_MODEL_ID` when `LLM_PROVIDER=bedrock`
- `GITHUB_APP_ID`
- `GITHUB_APP_PRIVATE_KEY`
- `GITHUB_APP_INSTALLATION_ID`
- `GITHUB_SEARCH_SCOPE_ORG`

Common optional variables:

- `PORT` (default: `3000`)
- `DATADOG_SITE` (default: `datadoghq.com`)
- `LANGUAGE` (default: `English`)

When `LLM_PROVIDER=bedrock`, AWS credentials are loaded from the standard AWS SDK environment or profile chain.

### 4. Configure Slack App

- Set Event Subscriptions Request URL to `https://<your-host>/slack/events`
- Subscribe to `app_mention`
- Grant the minimum Bot OAuth scopes required by the current implementation:
  `app_mentions:read`, `chat:write`, `channels:history`, `groups:history`
- Do not enable extra message event subscriptions or DM/private-conversation scopes unless you
  intentionally want to expand the support boundary beyond the current product policy

### 5. Run

Single-process runtime:

```bash
cd rust
bash -lc 'set -a; source ../.env; set +a; cargo run -p reili_runtime'
```

If you use `cargo-watch`:

```bash
cd rust
bash -lc 'set -a; source ../.env; set +a; cargo watch -x "run -p reili_runtime"'
```

## Usage

Mention the bot in Slack with an investigation request:

```text
@Reili Please investigate this alert. Check error increase in the last 30 minutes and correlate with recent PRs.
```

What happens:

1. It posts investigation progress in the thread
2. It investigates across Datadog and GitHub
3. It replies with an evidence-backed summary

## Permissions and Tool Transparency

Reili is intentionally scoped as a read-heavy investigation agent. In production it does not get
shell access, cluster access, deployment credentials, or write-capable GitHub/Datadog tools.
Its effective capabilities are the integrations and tools wired in this runtime.

### Agent Tool Inventory

The investigation agent can call only the following tool families:

- Slack progress reporting: `report_progress`
- Datadog MCP reads: `search_datadog_services`, `search_datadog_logs`,
  `analyze_datadog_logs`, `search_datadog_metrics`, `get_datadog_metric`,
  `get_datadog_metric_context`, `search_datadog_events`, `search_datadog_monitors`,
  `search_datadog_incidents`
- GitHub reads: `search_github_code`, `search_github_repos`,
  `search_github_issues_and_pull_requests`, `get_repository_content`, `get_pull_request`,
  `get_pull_request_diff`
- External web lookup: `search_web`

No tool is registered for GitHub writes, Slack admin actions, Datadog mutations, remediation, or
deployments.

### Slack Permissions and API Usage

Reili uses Slack as both its entry point and its reporting surface.

Required Slack credentials:

- `SLACK_SIGNING_SECRET`: verifies incoming requests on `/slack/events`
- `SLACK_BOT_TOKEN`: calls Slack Web API methods

Required Event Subscriptions:

- `app_mention`: starts an investigation when someone mentions the bot

Required Bot OAuth scopes:

- `app_mentions:read`: receive `app_mention` events
- `chat:write`: post progress and final replies into the originating thread
- `channels:history`: read public channel thread history when additional context is needed
- `groups:history`: read private channel thread history when additional context is needed

Not in scope for Slack:

- Additional non-mention message triggers such as `message.channels`
- Direct messages (`im`)
- Group direct messages (`mpim`)
- Their related event subscriptions and history scopes

Slack API methods currently used by the runtime:

- `auth.test`: resolves the bot user ID at startup
- `conversations.replies`: loads thread context when the triggering message is a thread reply
- `chat.postMessage`: posts queue failures and the final investigation summary
- `chat.startStream`, `chat.appendStream`, `chat.stopStream`: posts incremental investigation
  progress in the same thread

Slack boundary:

- Reads only the thread where the request was made, and only when additional thread context is
  needed
- Posts only into the originating thread
- Intended for channel conversations where the app is present, including public and private
  channels; DM and group DM usage are out of scope
- Does not delete messages, edit arbitrary messages, read files, manage channels, or administer the
  workspace

### Datadog Permissions and API Usage

Reili uses Datadog as a read-only evidence source during investigations.

Required Datadog credentials:

- `DATADOG_API_KEY`: sent as `DD_API_KEY`
- `DATADOG_APP_KEY`: sent as `DD_APPLICATION_KEY`
- `DATADOG_SITE`: controls the Datadog hostname such as `datadoghq.com`

Datadog capabilities currently used by the runtime:

- Connect to the remote Datadog MCP Server over Streamable HTTP
- Search services, logs, metrics, monitors, incidents, and events through Datadog-provided MCP
  tools
- Fetch metric detail and context from Datadog MCP for investigation pivots

Datadog MCP endpoint currently used by the runtime:

- `https://mcp.<DATADOG_SITE>/api/unstable/mcp-server/mcp?toolsets=core`

Recommended Datadog access policy:

- Create a dedicated Datadog service account for Reili
- Issue the API key and application key for that service account rather than reusing human
  operator credentials
- Prefer restricted application keys when your Datadog plan supports them
- Allow read access only to the Datadog products Reili actually uses through the `core` MCP
  toolset

Datadog boundary:

- No Datadog write endpoints are called by this runtime
- It does not create or edit monitors, dashboards, notebooks, SLOs, incidents, downtimes, or
  service definitions
- It does not acknowledge alerts, mute monitors, change retention, or trigger remediation actions
- Requests are scoped to investigation queries generated from the Slack thread context and user
  prompt

### GitHub App Permissions and Scope

Reili uses a GitHub App installation token and keeps GitHub access read-only.

Recommended GitHub App permissions:

- Repository metadata: read-only
- Contents: read-only
- Pull requests: read-only
- Issues: read-only

GitHub capabilities currently used by the runtime:

- Search repositories, code, issues, and pull requests
- Read repository files or directory listings for a given path and ref
- Read pull request metadata
- Read pull request diffs

GitHub boundary:

- No GitHub write permissions are required
- No commenting, merging, labeling, reviewing, or workflow dispatch is performed
- All GitHub search queries must stay inside `GITHUB_SEARCH_SCOPE_ORG`
- Repository content and pull request reads are rejected when the owner is outside
  `GITHUB_SEARCH_SCOPE_ORG`

### LLM Boundary

The configured LLM provider receives the Slack request, any loaded thread context, and the evidence
returned by the enabled tools so it can synthesize a report. When `search_web` is used, the query
is sent through the configured LLM provider's web-search capability to check external service
status or public incident reports.

## Development

For local development setup, architecture rules, and contributor workflows, see [DEVELOPERS.md](./DEVELOPERS.md).

## Non-Goals

- Executing operational actions like auto-remediation or auto-deploy
- Heavy stateful workflow orchestration

This project is intentionally focused on investigation and decision support.
