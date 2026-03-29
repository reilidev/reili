# Permissions and Boundaries

This document lists the current runtime's tool inventory, required integration permissions, and
behavioral boundaries.

Reili is intentionally scoped around task execution and decision support. The current runtime
is investigation-focused. It can post progress and final replies in Slack, but it does not get
shell access, cluster access, or deployment credentials in production. Its effective capabilities
are the integrations and tools wired in this runtime.

## Agent Tool Inventory

The task runner can call only the following tool families:

- Slack progress reporting: `report_progress` (primarily used to post progress messages back to Slack)
- Datadog MCP reads: `search_datadog_services`, `search_datadog_logs`, `analyze_datadog_logs`,
  `search_datadog_metrics`, `get_datadog_metric`, `get_datadog_metric_context`,
  `search_datadog_events`, `search_datadog_monitors`, `search_datadog_incidents`
- GitHub reads: `search_github_code`, `search_github_repos`, `search_github_issues_and_pull_requests`,
  `get_repository_content`, `get_pull_request`, `get_pull_request_diff`
- External web lookup: `search_web`

In the current runtime, no tool is registered for GitHub writes, Slack admin actions, Datadog
mutations, remediation, or deployments.

## Slack Permissions and API Usage

Reili uses Slack as both its entry point and its reporting surface.

Required Slack credentials:

- `SLACK_BOT_TOKEN`: calls Slack Web API methods
- `SLACK_SIGNING_SECRET`: verifies incoming requests on `/slack/events` in HTTP mode
- `SLACK_APP_TOKEN`: opens the Socket Mode WebSocket in Socket Mode (`xapp-...` App-Level Token)

Required Event Subscriptions:

- Enable Events
- `app_mention`: starts a task when someone mentions the bot

Configure the Slack app in two steps:

1. Apply the common settings below
2. Choose exactly one runtime mode: default `Socket Mode` or explicit `HTTP mode`

Common settings for both modes:

| Slack screen                          | Required setting                                                                                                   | Why                                                                                                              |
|---------------------------------------|--------------------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------|
| `OAuth & Permissions`                 | Add Bot Token Scopes: `app_mentions:read`, `chat:write`, `reactions:write`, `channels:history`, `groups:history` | Receive `app_mention`, mark accepted requests, reply in threads, and read channel/private-channel thread context |
| `Event Subscriptions`                 | Turn on events and add the bot event `app_mention`                                                                 | `app_mention` is the intake trigger in both modes                                                                |
| `Install App` / `OAuth & Permissions` | Install or reinstall the app after any scope change                                                                | Slack does not apply updated scopes until reinstall                                                              |
| Slack workspace                       | Invite the app to every public or private channel where it should respond                                          | The app must be present in the conversation to receive mentions and post replies                                 |

Required Slack app settings for Socket Mode:

- Enable Socket Mode
- Create an App-Level Token with the `connections:write` scope
- Set `SLACK_APP_TOKEN` to that App-Level Token (`xapp-...`)
- Leave `SLACK_SOCKET_MODE` unset or set it to `true`
- Set `SLACK_BOT_TOKEN` to the installed bot token (`xoxb-...`)
- Keep `Event Subscriptions` enabled; Socket Mode replaces the Request URL, not the event subscription itself
- Do not configure an `Event Subscriptions` Request URL; Socket Mode does not use it

Required Slack app settings for HTTP mode:

- Set `SLACK_SOCKET_MODE=false`
- Set `SLACK_BOT_TOKEN`
- Set `SLACK_SIGNING_SECRET`
- In `Event Subscriptions`, configure the Request URL as `https://<your-host>/slack/events`
- Do not create or set `SLACK_APP_TOKEN`
- Do not enable Socket Mode

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
- `chat.startStream`, `chat.appendStream`, `chat.stopStream`: posts incremental task progress in the same thread

Slack boundary:

- Reads only the thread where the request was made, and only when additional thread context is needed
- Posts only into the originating thread
- Intended for channel conversations where the app is present, including public and private channels; DM and group DM
  usage are out of scope
- Does not delete messages, edit arbitrary messages, read files, manage channels, or administer the workspace

## Datadog Permissions and API Usage

Reili uses Datadog as an evidence source during current investigation tasks.

Required Datadog credentials:

- `DATADOG_API_KEY`: sent as `DD_API_KEY`
- `DATADOG_APP_KEY`: sent as `DD_APPLICATION_KEY`; this is the credential that carries the Datadog RBAC permissions
  listed below
- `DATADOG_SITE`: controls the Datadog hostname such as `datadoghq.com`

Required Datadog permissions:

- `mcp_read`: required because Reili connects to the remote Datadog MCP Server in read-only mode
  ([Datadog RBAC permissions](https://docs.datadoghq.com/account_management/rbac/permissions/))
- `logs_read_data`: required for `search_datadog_logs` and `analyze_datadog_logs`, and for the direct log search and
  log aggregate APIs used by the runtime ([Logs RBAC permissions](https://docs.datadoghq.com/logs/guide/logs-rbac-permissions/))
- `logs_read_index_data`: also required for indexed log access when your organization still uses index-scoped log
  permissions ([Logs RBAC permissions](https://docs.datadoghq.com/logs/guide/logs-rbac-permissions/))
- `metrics_read`: required for metric catalog and metric metadata/context reads
  ([Metrics API docs](https://docs.datadoghq.com/api/latest/metrics/))
- `timeseries_query`: required for Datadog timeseries metric queries
  ([Metrics API docs](https://docs.datadoghq.com/api/latest/metrics/))
- `events_read`: required for Datadog event search ([Events API docs](https://docs.datadoghq.com/api/latest/events/))
- `monitors_read`: required for Datadog monitor search
  ([Monitors API docs](https://docs.datadoghq.com/api/latest/monitors/))
- `incident_read`: required for Datadog incident search
  ([Incidents API docs](https://docs.datadoghq.com/api/latest/incidents/))
- `apm_service_catalog_read`: recommended for `search_datadog_services` when service discovery is backed by Datadog
  Service Catalog ([Software Catalog permission docs](https://docs.datadoghq.com/internal_developer_portal/software_catalog/set_up/))
- `apm_read`: may also be required for `search_datadog_services` in organizations where the available service inventory
  comes from APM service reads rather than Service Catalog definitions
  ([APM API docs](https://docs.datadoghq.com/api/latest/apm/), [Datadog RBAC permissions](https://docs.datadoghq.com/account_management/rbac/permissions/))

In practice, the API key authenticates the Datadog organization, while the application key
determines which read operations Reili can perform. Create the application key from a dedicated
Datadog service account and grant that account only the permissions above.

Datadog capabilities currently used by the runtime:

- Connect to the remote Datadog MCP Server over Streamable HTTP
- Search services, logs, metrics, monitors, incidents, and events through Datadog-provided MCP tools
- Fetch metric detail and context from Datadog MCP for task pivots

Datadog MCP endpoint currently used by the runtime:

- `https://mcp.<DATADOG_SITE>/api/unstable/mcp-server/mcp?toolsets=core`

Recommended Datadog access policy:

- Create a dedicated Datadog service account for Reili
- Issue the API key and application key for that service account rather than reusing human operator credentials
- Prefer restricted application keys when your Datadog plan supports them
- Allow only the minimum Datadog product access Reili needs through the `core` MCP toolset

Datadog boundary:

- No Datadog write endpoints are called by this runtime
- It does not create or edit monitors, dashboards, notebooks, SLOs, incidents, downtimes, or service definitions
- It does not acknowledge alerts, mute monitors, change retention, or trigger remediation actions
- Requests are scoped to task queries generated from the Slack thread context and user prompt

## GitHub App Permissions and Scope

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
- Repository content and pull request reads are rejected when the owner is outside `GITHUB_SEARCH_SCOPE_ORG`

## LLM Boundary

The configured LLM provider receives the Slack request, any loaded thread context, and the evidence
returned by the enabled tools so it can synthesize a report. When `search_web` is used, the query
is sent through the configured LLM provider's web-search capability to check external service
status or public incident reports.
