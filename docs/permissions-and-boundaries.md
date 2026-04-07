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
- Slack workspace lookup: `search_slack_messages` (searches prior Slack public-channel messages visible to the current invocation context)
- Datadog MCP reads: `search_datadog_services`, `search_datadog_logs`, `analyze_datadog_logs`,
  `search_datadog_metrics`, `get_datadog_metric`, `get_datadog_metric_context`,
  `search_datadog_events`, `search_datadog_monitors`, `search_datadog_incidents`
- GitHub MCP reads: `search_code`, `search_repositories`, `search_issues`,
  `search_pull_requests`, `get_file_contents`, `pull_request_read`
- External web lookup: `search_web`

In the current runtime, no tool is registered for GitHub writes, Slack admin actions, Datadog
mutations, remediation, or deployments.

## Slack Permissions and API Usage

Reili uses Slack as both its entry point and its reporting surface.

Required Slack credentials:

- `SLACK_BOT_TOKEN`: calls Slack Web API methods
- `SLACK_SIGNING_SECRET`: verifies incoming requests on `/slack/events` in HTTP mode
- `SLACK_APP_TOKEN`: opens the Socket Mode WebSocket in Socket Mode (`xapp-...` App-Level Token)

Additional Slack platform requirement:

- `assistant.search.context` is currently available only to internal Slack apps or apps distributed
  through Slack Marketplace/App Directory
- In Slack App settings, open `Agents & AI Apps` and enable `Agent or Assistant` so Slack agent
  search capabilities are available to the app
- The current runtime uses the Bot Token + `action_token` flow, which supports public-channel
  message search through `search:read.public`

Required Event Subscriptions:

- Enable Events
- `app_mention`: starts a task when someone mentions the bot and carries the `action_token` used by `assistant.search.context`

Configure the Slack app in two steps:

1. Apply the common settings below
2. Choose exactly one runtime mode: default `Socket Mode` or explicit `HTTP mode`

Common settings for both modes:

| Slack screen                          | Required setting                                                                                                   | Why                                                                                                              |
|---------------------------------------|--------------------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------|
| `Agents & AI Apps`                    | Turn on `Agent or Assistant`                                                                                       | Enables Slack agent search capabilities such as `assistant.search.context`                                       |
| `OAuth & Permissions`                 | Add Bot Token Scopes: `app_mentions:read`, `chat:write`, `reactions:write`, `channels:history`, `search:read.public` | Receive `app_mention`, mark accepted requests, reply in threads, read channel thread context, and search Slack public-channel messages |
| `Event Subscriptions`                 | Turn on events and add the bot event `app_mention`                                                                 | `app_mention` is the intake trigger in both modes                                                                |
| `Interactivity & Shortcuts`           | Turn on interactivity                                                                                              | Receive `block_actions` when a user clicks a task `Cancel` button                                                |
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
- In `Interactivity & Shortcuts`, configure the Request URL as `https://<your-host>/slack/interactions`
- Do not create or set `SLACK_APP_TOKEN`
- Do not enable Socket Mode

Required Bot OAuth scopes:

- `app_mentions:read`: receive `app_mention` events
- `chat:write`: post progress and final replies into the originating thread, and post/update the task control message
- `reactions:write`: add an `:eyes:` reaction to the triggering message once the task is queued
- `channels:history`: read public channel thread history when additional context is needed
- `search:read.public`: call `assistant.search.context` for public-channel Slack message search with a Bot Token

Required App-Level Token scope for Socket Mode:

- `connections:write`: call `apps.connections.open` to obtain the temporary WebSocket URL

Not in scope for Slack:

- Additional non-mention message triggers such as `message.channels`
- Direct messages (`im`)
- Group direct messages (`mpim`)
- Their related event subscriptions and history scopes

Slack API methods currently used by the runtime:

- `apps.connections.open`: obtains a temporary WebSocket URL when Socket Mode is enabled
- `assistant.search.context`: searches Slack public-channel message history using the triggering event's `action_token`
- `auth.test`: resolves the bot user ID at startup
- `conversations.replies`: loads thread context when the triggering message is a thread reply
- `chat.postMessage`: posts queue failures, the final task summary, and the task control message
- `chat.update`: updates the task control message as tasks move through running/cancelled/completed/failed states
- `reactions.add`: adds an `:eyes:` reaction to the triggering message after enqueue succeeds
- `chat.startStream`, `chat.appendStream`, `chat.stopStream`: posts incremental task progress in the same thread

Slack boundary:

- Reads only the thread where the request was made, and only when additional thread context is needed
- Searches only Slack public-channel messages permitted by the current app install, bot token scope, and `action_token` context
- Posts only into the originating thread
- Intended for channel conversations where the app is present, including public and private channels; DM and group DM
  usage are out of scope
- Does not search private channels or DMs with the current Bot Token configuration
- Does not delete messages, edit arbitrary messages, read files, manage channels, or administer the workspace

## Datadog Permissions and API Usage

Reili uses Datadog as an evidence source during current investigation tasks.
Its Datadog integration surface is the Datadog-hosted remote MCP Server, and the runtime uses
Datadog-provided read-only MCP tools rather than a separate direct Datadog client layer.

Required Datadog credentials:

- `DATADOG_API_KEY`: sent as `DD_API_KEY`
- `DATADOG_APP_KEY`: sent as `DD_APPLICATION_KEY`; this is the credential that carries the Datadog RBAC permissions
  listed below
- `DATADOG_SITE`: controls the Datadog hostname such as `datadoghq.com`

Required Datadog permissions:

- `mcp_read`: required because Reili connects to the remote Datadog MCP Server in read-only mode
  ([Datadog RBAC permissions](https://docs.datadoghq.com/account_management/rbac/permissions/))
- Additional product read permissions depend on which Datadog MCP tools your organization exposes
  through the enabled `core` toolset.
- `logs_read_data` and, in some organizations, `logs_read_index_data`: needed when Reili uses
  `search_datadog_logs` or `analyze_datadog_logs`
  ([Logs RBAC permissions](https://docs.datadoghq.com/logs/guide/logs-rbac-permissions/))
- `metrics_read` and `timeseries_query`: needed when Reili uses metric search, metric detail, or
  metric context MCP tools ([Metrics API docs](https://docs.datadoghq.com/api/latest/metrics/))
- `events_read`: needed when Reili uses `search_datadog_events`
  ([Events API docs](https://docs.datadoghq.com/api/latest/events/))
- `monitors_read`: needed when Reili uses `search_datadog_monitors`
  ([Monitors API docs](https://docs.datadoghq.com/api/latest/monitors/))
- `incident_read`: needed when Reili uses `search_datadog_incidents`
  ([Incidents API docs](https://docs.datadoghq.com/api/latest/incidents/))
- `apm_service_catalog_read` or `apm_read`: may be needed when Reili uses `search_datadog_services`,
  depending on how service inventory is backed in your Datadog organization
  ([Software Catalog permission docs](https://docs.datadoghq.com/internal_developer_portal/software_catalog/set_up/),
  [APM API docs](https://docs.datadoghq.com/api/latest/apm/),
  [Datadog RBAC permissions](https://docs.datadoghq.com/account_management/rbac/permissions/))

In practice, the API key authenticates the Datadog organization, while the application key
determines which MCP-backed read operations Reili can perform. Create the application key from a
dedicated Datadog service account and grant that account only the minimum read permissions above.

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

## GitHub Permissions and Scope

Reili reads GitHub through GitHub MCP over Streamable HTTP.

Runtime authentication model:

- Reili signs a GitHub App JWT with `GITHUB_APP_PRIVATE_KEY`
- Reili exchanges that JWT for a short-lived installation token using `GITHUB_APP_ID` and `GITHUB_APP_INSTALLATION_ID`
- The resulting installation token is used as the GitHub MCP bearer token and refreshed before expiry

Recommended permissions for the current runtime:

- GitHub App permissions that can read the repositories Reili investigates
- Prefer a GitHub App and MCP server configuration that exposes only the minimum read capabilities Reili needs

GitHub capabilities currently used by the runtime:

- Search repositories, code, issues, and pull requests
- Read repository files or directory listings for a given path and ref
- Read pull request metadata
- Read pull request diffs

GitHub MCP boundary:

- The GitHub specialist agent only receives this allowlisted subset of MCP tools:
  `search_code`, `search_repositories`, `search_issues`, `search_pull_requests`,
  `get_file_contents`, and `pull_request_read`
- It does not mint or refresh GitHub MCP tokens; any short-lived token rotation must happen outside the runtime
- Reili still enforces `GITHUB_SEARCH_SCOPE_ORG` before allowlisted GitHub MCP tool calls are made
- Raw MCP write tools are not exposed to the GitHub agent in this

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
