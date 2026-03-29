<div align="center">
  <h1>Reili</h1>
  <img src="./reili.png" alt="Reili logo" width="240" />
  <p><strong>A Slack-native AI agent for DevOps tasks, currently focused on investigations</strong></p>
  <p>
    Investigate alerts quickly across Datadog, GitHub, and Slack threads.
    <br />
    Operate with a simple database-free architecture.
  </p>
</div>

## Why Reili

`Reili` starts from Slack messages and task requests, then:

- Investigates Datadog Logs, Metrics, and Events
- Explores GitHub repositories, PRs, Issues, and code while connecting that context with Datadog to understand system structure and trace issues

Its current task focus is triage, investigation, and communicating findings.

## Core Features

- Slack-native intake via `app_mention` events
- Task result reporting in Slack threads
- Evidence collection from GitHub and Datadog

### Runtime Characteristics

- Database-free: no persistent state component
- Job queue is in-memory, so pending jobs are lost on app restart

## Quick Start

### 1. Prerequisites

- Slack App (Bot Token, plus Signing Secret for HTTP mode or App-Level Token for Socket Mode)
- Datadog API Key + APP Key
- OpenAI API Key, AWS credentials with permission to use Amazon Bedrock, or Google Cloud ADC with permission to call Vertex AI partner models
- GitHub App (App ID / Private Key / Installation ID)

### 2. Configure Environment Variables

Required:

- `SLACK_BOT_TOKEN`
- `SLACK_SIGNING_SECRET` when `SLACK_SOCKET_MODE` is unset or `false` (HTTP mode)
- `SLACK_SOCKET_MODE=true` and `SLACK_APP_TOKEN` when using Socket Mode
- `DATADOG_API_KEY`
- `DATADOG_APP_KEY`
- `LLM_PROVIDER`
- `LLM_OPENAI_API_KEY` when `LLM_PROVIDER=openai`
- `LLM_BEDROCK_MODEL_ID` when `LLM_PROVIDER=bedrock`
- `LLM_VERTEX_AI_MODEL_ID` when `LLM_PROVIDER=vertexai`
- `GOOGLE_CLOUD_LOCATION` when `LLM_PROVIDER=vertexai`
- `GOOGLE_CLOUD_PROJECT` when `LLM_PROVIDER=vertexai`
- `GITHUB_APP_ID`
- `GITHUB_APP_PRIVATE_KEY`
- `GITHUB_APP_INSTALLATION_ID`
- `GITHUB_SEARCH_SCOPE_ORG`

Common optional variables:

- `PORT` (default: `3000`)
- `DATADOG_SITE` (default: `datadoghq.com`)
- `LANGUAGE` (default: `English`)

`SLACK_APP_TOKEN` must be a Slack App-Level Token that starts with `xapp-`. When Socket Mode is
enabled, `SLACK_SIGNING_SECRET` is not used.

When `LLM_PROVIDER=bedrock`, AWS credentials and region are loaded from the standard AWS SDK environment or profile chain. Set `AWS_PROFILE` to use a named AWS profile such as an AWS SSO profile, and set `AWS_REGION` if the selected profile does not already define a region.

When `LLM_PROVIDER=vertexai`, Google credentials are loaded from Application Default Credentials.

- Set `GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION`, and `LLM_VERTEX_AI_MODEL_ID`.
- Use the exact Vertex AI Anthropic model id, including the published version suffix when Google provides one.
- If Vertex AI returns `RESOURCE_EXHAUSTED`, verify your project quotas in Google Cloud Quotas (`https://console.cloud.google.com/iam-admin/quotas`) and adjust them if needed.

### 3. Configure Slack App

Configure the Slack app in two steps:

1. Apply the common settings below
2. Choose exactly one runtime mode: `Socket Mode` or `HTTP mode`

Common settings for both modes:

| Slack screen | Required setting | Why |
| --- | --- | --- |
| `OAuth & Permissions` | Add Bot Token Scopes: `app_mentions:read`, `chat:write`, `reactions:write` (required for the reaction indicator), `channels:history`, `groups:history` | Receive `app_mention`, mark accepted requests, reply in threads, and read channel/private-channel thread context |
| `Event Subscriptions` | Turn on events and add the bot event `app_mention` | `app_mention` is the intake trigger in both modes |
| `Install App` / `OAuth & Permissions` | Install or reinstall the app after any scope change | Slack does not apply updated scopes until reinstall |
| Slack workspace | Invite the app to every public or private channel where it should respond | The app must be present in the conversation to receive mentions and post replies |

If you choose `Socket Mode` (`SLACK_SOCKET_MODE=true`):

- Enable `Socket Mode` in the Slack app settings
- In `Basic Information`, create an App-Level Token with the `connections:write` scope
- Set `SLACK_APP_TOKEN` to that App-Level Token (`xapp-...`)
- Set `SLACK_BOT_TOKEN` to the installed bot token (`xoxb-...`)
- Keep `Event Subscriptions` enabled with `app_mention`
- Do not configure an `Event Subscriptions Request URL`; Socket Mode does not use it

If you choose `HTTP mode` (`SLACK_SOCKET_MODE` unset or `false`):

- Set `SLACK_BOT_TOKEN`
- Set `SLACK_SIGNING_SECRET`
- In `Event Subscriptions`, configure the Request URL as `https://<your-host>/slack/events`
- Do not create or set `SLACK_APP_TOKEN`
- Do not enable Socket Mode

Current product boundary:

- `app_mention` does not cover direct messages; DM and group DM support remain out of scope
- Do not enable extra message event subscriptions or DM/private-conversation scopes unless you intentionally want to expand the support boundary beyond the current product policy

## Usage

Mention the bot in Slack with a task request:

```text
@Reili Please investigate this alert. Check error increase in the last 30 minutes and correlate with recent PRs.
```

What happens:

1. It posts task progress in the thread
2. It investigates across Datadog and GitHub
3. It replies with an evidence-backed summary

## Permissions and Tool Transparency

Reili is intentionally scoped around task execution and decision support. The current runtime remains read-only and investigation-focused. In production it does not
get shell access, cluster access, or deployment credentials. Its effective capabilities are the
integrations and tools wired in this runtime.

### Agent Tool Inventory

The task runner can call only the following tool families:

- Slack progress reporting: `report_progress` (primarily used to post progress messages back to Slack)
- Datadog MCP reads: `search_datadog_services`, `search_datadog_logs`,
  `analyze_datadog_logs`, `search_datadog_metrics`, `get_datadog_metric`,
  `get_datadog_metric_context`, `search_datadog_events`, `search_datadog_monitors`,
  `search_datadog_incidents`
- GitHub reads: `search_github_code`, `search_github_repos`,
  `search_github_issues_and_pull_requests`, `get_repository_content`, `get_pull_request`,
  `get_pull_request_diff`
- External web lookup: `search_web`

In the current runtime, no tool is registered for GitHub writes, Slack admin actions, Datadog
mutations, remediation, or deployments.

### Slack Permissions and API Usage

Reili uses Slack as both its entry point and its reporting surface.

Required Slack credentials:

- `SLACK_BOT_TOKEN`: calls Slack Web API methods
- `SLACK_SIGNING_SECRET`: verifies incoming requests on `/slack/events` in HTTP mode
- `SLACK_APP_TOKEN`: opens the Socket Mode WebSocket in Socket Mode (`xapp-...` App-Level Token)

Required Event Subscriptions:

- Enable Events
- `app_mention`: starts a task when someone mentions the bot

Required Slack app settings for Socket Mode:

- Enable Socket Mode
- Create an App-Level Token with the `connections:write` scope
- Keep `Event Subscriptions` enabled; Socket Mode replaces the Request URL, not the event subscription itself

Required Bot OAuth scopes:

- `app_mentions:read`: receive `app_mention` events
- `chat:write`: post progress and final replies into the originating thread
- `reactions:write`: add an `:eyes:` reaction to the triggering message once the task is queued
- `channels:history`: read public channel thread history when additional context is needed
- `groups:history`: read private channel thread history when additional context is needed

Required App-Level Token scope for Socket Mode:

- `connections:write`: call `apps.connections.open` to obtain the temporary WebSocket URL

Not in scope for Slack:

- Additional non-mention message triggers such as `message.channels`
- Direct messages (`im`)
- Group direct messages (`mpim`)
- Their related event subscriptions and history scopes

Slack API methods currently used by the runtime:

- `apps.connections.open`: obtains a temporary WebSocket URL when Socket Mode is enabled
- `auth.test`: resolves the bot user ID at startup
- `conversations.replies`: loads thread context when the triggering message is a thread reply
- `chat.postMessage`: posts queue failures and the final task summary
- `reactions.add`: adds an `:eyes:` reaction to the triggering message after enqueue succeeds
- `chat.startStream`, `chat.appendStream`, `chat.stopStream`: posts incremental task
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

Reili uses Datadog as an evidence source during current investigation tasks.

Required Datadog credentials:

- `DATADOG_API_KEY`: sent as `DD_API_KEY`
- `DATADOG_APP_KEY`: sent as `DD_APPLICATION_KEY`
- `DATADOG_SITE`: controls the Datadog hostname such as `datadoghq.com`

Datadog capabilities currently used by the runtime:

- Connect to the remote Datadog MCP Server over Streamable HTTP
- Search services, logs, metrics, monitors, incidents, and events through Datadog-provided MCP
  tools
- Fetch metric detail and context from Datadog MCP for task pivots

Datadog MCP endpoint currently used by the runtime:

- `https://mcp.<DATADOG_SITE>/api/unstable/mcp-server/mcp?toolsets=core`

Recommended Datadog access policy:

- Create a dedicated Datadog service account for Reili
- Issue the API key and application key for that service account rather than reusing human
  operator credentials
- Prefer restricted application keys when your Datadog plan supports them
- Allow only the minimum Datadog product access Reili needs through the `core` MCP toolset

Datadog boundary:

- No Datadog write endpoints are called by this runtime
- It does not create or edit monitors, dashboards, notebooks, SLOs, incidents, downtimes, or
  service definitions
- It does not acknowledge alerts, mute monitors, change retention, or trigger remediation actions
- Requests are scoped to task queries generated from the Slack thread context and user
  prompt

### GitHub App Permissions and Scope

Reili uses a GitHub App installation token. The current runtime only exercises read-only,
investigation-oriented task capabilities against GitHub.

Recommended GitHub App permissions for the current runtime:

- Repository metadata: read
- Contents: read
- Pull requests: read
- Issues: read

GitHub capabilities currently used by the runtime:

- Search repositories, code, issues, and pull requests
- Read repository files or directory listings for a given path and ref
- Read pull request metadata
- Read pull request diffs

GitHub boundary in the current runtime:

- No GitHub write permissions are required today
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

## Release

- Pull requests and pushes to `main` run `cargo fmt`, `cargo clippy`, `cargo test`, and a Docker build validation in GitHub Actions.
- `tagpr` maintains the release pull request on every push to `main`; merging that PR tags the merge commit, creates the `vX.Y.Z` GitHub Release, uploads Linux binary archives, and publishes a multi-architecture container image to `ghcr.io/<owner>/<repo>`.
- The release workflow tracks `crates/Cargo.toml` as the workspace version file, refreshes `crates/Cargo.lock` after version bumps, updates `CHANGELOG.md`, and injects the release tag into the binary at compile time as `APP_VERSION`.
- The release workflow mints a GitHub App installation token via `actions/create-github-app-token`; configure `vars.REILI_BOT_APP_ID` and `secrets.REILI_BOT_APP_PRIVATE_KEY` for that app.
- The container exposes `/healthz` for runtime health checks and listens on `PORT` (default `3000`).

## Non-Goals

- Executing operational actions like auto-remediation or auto-deploy
- Heavy stateful workflow orchestration

This project is intentionally focused on investigation-oriented task execution and decision support.
