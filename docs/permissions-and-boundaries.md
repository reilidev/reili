# Permissions and Boundaries

This document lists the current runtime's tool inventory, required integration permissions, and
behavioral boundaries.

Reili is intentionally scoped around task execution and decision support. The current runtime
is investigation-focused. It can post progress and final replies in Slack, but it does not get
shell access, cluster access, or deployment credentials in production. Its effective capabilities
are the integrations and tools wired in this runtime.

## Agent Tool Inventory

The runtime can expose only the following tool families:

- Slack progress reporting: `report_progress` (primarily used to post progress messages back to Slack)
- Slack workspace lookup and lightweight memory: `search_slack_messages` plus startup memory loading
  (searches prior Slack public-channel messages visible to the current invocation context)
- Datadog MCP reads: `search_datadog_services`, `search_datadog_logs`, `analyze_datadog_logs`,
  `search_datadog_metrics`, `get_datadog_metric`, `get_datadog_metric_context`,
  `search_datadog_events`, `search_datadog_monitors`, `search_datadog_dashboards`,
  `get_datadog_dashboard`, `get_synthetics_tests`
- Datadog MCP security reads when exposed by Datadog for the configured credentials:
  `search_datadog_security_signals`, `security_findings_schema`, `search_security_findings`,
  `analyze_security_findings`
- GitHub MCP reads: `search_code`, `search_repositories`, `search_issues`,
  `search_pull_requests`, `get_file_contents`, `pull_request_read`, `actions_get`,
  `actions_list`, `get_job_logs`, `get_dependabot_alert`, `list_dependabot_alerts`
- esa specialist delegation when `[connector.esa]` is configured: `investigate_esa`
- External web lookup: `search_web`

In the current runtime, no tool is registered for GitHub writes, Slack admin actions, Datadog
mutations, esa writes, remediation, or deployments.

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
| `OAuth & Permissions`                 | Add Bot Token Scopes: `app_mentions:read`, `chat:write`, `reactions:write`, `channels:history`, `channels:read`, `usergroups:read`, `search:read.public` | Receive `app_mention`, mark accepted requests, reply in threads, read channel thread context, resolve authorization allowlists, reject private-channel mentions, and search Slack public-channel messages |
| `Event Subscriptions`                 | Turn on events and add the bot event `app_mention`                                                                 | `app_mention` is the intake trigger in both modes                                                                |
| `Interactivity & Shortcuts`           | Turn on interactivity                                                                                              | Receive `block_actions` when a user clicks a task `Cancel` button                                                |
| `Install App` / `OAuth & Permissions` | Install or reinstall the app after any scope change                                                                | Slack does not apply updated scopes until reinstall                                                              |
| Slack workspace                       | Invite the app to every public channel where it should respond                                                     | The app must be present in the conversation to receive mentions and post replies                                 |

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
- `chat:write`: post progress and final replies into the originating thread, post/update the task control message, and post authorization deny notices as ephemeral messages
- `reactions:write`: add an `:eyes:` reaction to the triggering message once the task is queued
- `channels:history`: read public channel thread history when additional context is needed
- `channels:read`: resolve public channel metadata for mention authorization
- `usergroups:read`: resolve user group membership for mention authorization
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
- `assistant.search.context`: searches Slack public-channel message history using the triggering event's `action_token`;
  Reili uses this both for the `search_slack_messages` tool and for startup loading of recent
  Reili reusable notes marked with `reili_memory_v1`
- `auth.test`: resolves the bot user ID at startup
- `conversations.info`: resolves originating public channel metadata to evaluate channel name authorization patterns; private-channel lookup fails without `groups:read` and is denied before enqueue
- `conversations.replies`: loads thread context when the triggering message is a thread reply
- `chat.postMessage`: posts queue failures, the final task summary, and the task control message
- `chat.postEphemeral`: posts a private deny notice to the mentioning user when mention authorization rejects a request
- `chat.update`: updates the task control message as tasks move through running/cancelled/completed/failed states
- `usergroups.users.list`: resolves configured user group membership for mention authorization
- `reactions.add`: adds an `:eyes:` reaction to the triggering message after enqueue succeeds
- `chat.startStream`, `chat.appendStream`, `chat.stopStream`: posts incremental task progress in the same thread

Slack boundary:

- Private channels are not supported; `app_mention` events from private channels are denied before enqueue
- `groups:read` is intentionally not requested; private channel metadata is not read
- When configured, handles only `app_mention` events matching the Slack channel name, user ID, user group, or bot actor authorization settings
- Reads only the thread where the request was made, and only when additional thread context is needed
- Searches only Slack public-channel messages permitted by the current app install, bot token scope, and `action_token` context
- Loads lightweight memory only from Reili bot-authored Slack replies in the current public channel
  when they contain the `reili_memory_v1` marker
- Posts only into the originating thread
- Intended for public channel conversations where the app is present; private channels, DM, and group DM usage are out of scope
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
  through Reili's internal `core,security,dashboards,synthetics` MCP request.
- `logs_read_data` and, in some organizations, `logs_read_index_data`: needed when Reili uses
  `search_datadog_logs` or `analyze_datadog_logs`
  ([Logs RBAC permissions](https://docs.datadoghq.com/logs/guide/logs-rbac-permissions/))
- `metrics_read` and `timeseries_query`: needed when Reili uses metric search, metric detail, or
  metric context MCP tools ([Metrics API docs](https://docs.datadoghq.com/api/latest/metrics/))
- `events_read`: needed when Reili uses `search_datadog_events`
  ([Events API docs](https://docs.datadoghq.com/api/latest/events/))
- `monitors_read`: needed when Reili uses `search_datadog_monitors`
  ([Monitors API docs](https://docs.datadoghq.com/api/latest/monitors/))
- `Dashboards Read` and `User Access Read`: needed when Reili uses
  `search_datadog_dashboards` or `get_datadog_dashboard`
- `apm_service_catalog_read` or `apm_read`: may be needed when Reili uses `search_datadog_services`,
  depending on how service inventory is backed in your Datadog organization
  ([Software Catalog permission docs](https://docs.datadoghq.com/internal_developer_portal/software_catalog/set_up/),
  [APM API docs](https://docs.datadoghq.com/api/latest/apm/),
  [Datadog RBAC permissions](https://docs.datadoghq.com/account_management/rbac/permissions/))
- `Synthetics Read`: needed when Reili uses `get_synthetics_tests`
- `security_monitoring_signals_read`: needed when Reili uses `search_datadog_security_signals`
- `security_monitoring_findings_read`: needed when Reili uses
  `security_findings_schema`, `search_security_findings`, or `analyze_security_findings`
- `timeseries_query`: also needed when Reili uses `analyze_security_findings`

In practice, the API key authenticates the Datadog organization, while the application key
determines which MCP-backed read operations Reili can perform. Create the application key from a
dedicated Datadog service account and grant that account only the minimum read permissions above.

Reili always requests Datadog MCP with `toolsets=core,security,dashboards,synthetics`
internally. This does not add a new runtime config setting. Datadog still decides which tools are
returned based on your plan, org configuration, and application key permissions.

Datadog capabilities currently used by the runtime:

- Connect to the remote Datadog MCP Server over Streamable HTTP
- Search services, logs, metrics, monitors, and events through Datadog-provided MCP tools
- Search dashboards, retrieve full dashboard definitions, and query Synthetic tests through
  Datadog-provided MCP tools
- Search security signals and investigate security findings when Datadog returns those read tools
- Fetch metric detail and context from Datadog MCP for task pivots

Datadog MCP endpoint currently used by the runtime:

- `https://mcp.<DATADOG_SITE>/api/unstable/mcp-server/mcp?toolsets=core,security,dashboards,synthetics`

Recommended Datadog access policy:

- Create a dedicated Datadog service account for Reili
- Issue the API key and application key for that service account rather than reusing human operator credentials
- Prefer restricted application keys when your Datadog plan supports them
- Allow only the minimum Datadog product access Reili needs across the allowlisted `core`,
  `security`, `dashboards`, and `synthetics` read tools
- Reili exposes only the intersection of its allowlisted Datadog tools and the tools Datadog
  actually returns for your credentials
- If Datadog omits allowlisted tools, Reili logs a warning and continues with the remaining
  available Datadog tools

Datadog boundary:

- No Datadog write endpoints are called by this runtime
- From the Datadog `security`, `dashboards`, and `synthetics` toolsets, Reili allowlists only
  read-only investigation tools
- It does not create or edit monitors, dashboards, notebooks, Synthetic tests, SLOs, incidents, downtimes, or service definitions
- It does not acknowledge alerts, mute monitors, change retention, or trigger remediation actions
- Requests are scoped to task queries generated from the Slack thread context and user prompt

## GitHub Permissions and Scope

Reili reads GitHub through GitHub MCP over Streamable HTTP.

Runtime authentication model:

- Reili signs a GitHub App JWT with `GITHUB_APP_PRIVATE_KEY`
- Reili exchanges that JWT for a short-lived installation token using `GITHUB_APP_ID` and `GITHUB_APP_INSTALLATION_ID`
- The resulting installation token is used as the GitHub MCP bearer token and refreshed before expiry

Recommended permissions for the current runtime:

- GitHub App repository permissions: `Metadata` (read), `Contents` (read), `Pull requests` (read),
  `Issues` (read), `Actions` (read), and `Dependabot alerts` (read)
- Prefer a GitHub App and MCP server configuration that exposes only the minimum read capabilities Reili needs

GitHub MCP toolset request:

- Reili sends `X-MCP-Toolsets: default,actions,dependabot` when connecting to GitHub MCP
- If your GitHub MCP deployment ignores remote toolset headers, configure the server itself to
  expose equivalent toolsets so the Actions and Dependabot read tools are available

GitHub capabilities currently used by the runtime:

- Search repositories, code, issues, and pull requests
- Read repository files or directory listings for a given path and ref
- Read pull request metadata
- Read pull request diffs
- Inspect GitHub Actions workflows, runs, jobs, and job logs
- Read Dependabot alert summaries and alert details

GitHub MCP boundary:

- The GitHub specialist agent only receives this allowlisted subset of MCP tools:
  `search_code`, `search_repositories`, `search_issues`, `search_pull_requests`,
  `get_file_contents`, `pull_request_read`, `actions_get`, `actions_list`, `get_job_logs`,
  `get_dependabot_alert`, and `list_dependabot_alerts`
- It does not mint or refresh GitHub MCP tokens; any short-lived token rotation must happen outside the runtime
- Reili still enforces `GITHUB_SEARCH_SCOPE_ORG` before allowlisted GitHub MCP tool calls are made
- Raw MCP write tools are not exposed to the GitHub agent in this runtime

GitHub boundary in the current runtime:

- No GitHub write permissions are required today
- No commenting, merging, labeling, reviewing, or workflow dispatch is performed
- All GitHub search queries must stay inside `GITHUB_SEARCH_SCOPE_ORG`
- Repository content, pull request, Actions, and Dependabot reads are rejected when the owner is
  outside `GITHUB_SEARCH_SCOPE_ORG`

## esa Permissions and Scope

Reili can optionally search an esa team's posts as an internal documentation source.
This integration is disabled unless `[connector.esa]` is present in `reili.toml`.
When the section is omitted, Reili does not read `ESA_ACCESS_TOKEN` and does not register
`investigate_esa` or `search_posts`.

Required esa credential when configured:

- `ESA_ACCESS_TOKEN`: an esa access token with the `read` scope for the configured team

Required `reili.toml` fields when configured:

- `connector.esa.team_name`: esa team name, e.g. `docs` for `https://docs.esa.io/`
- `connector.esa.access_token_env`: optional env var name; defaults to `ESA_ACCESS_TOKEN`

esa API endpoint currently used by the runtime:

- `GET /v1/teams/:team_name/posts`

esa capabilities currently used by the runtime:

- Delegate documentation investigation to the `investigate_esa` specialist agent
- Search posts through the specialist's `search_posts` tool using esa's `q` search syntax
- Return post metadata, links, tags, authors, pagination metadata, and Markdown body content from
  the search response

esa boundary:

- No esa write endpoints are called by this runtime
- Reili does not create, edit, delete, star, watch, archive, share, or comment on esa posts
- Requests are scoped to the single configured esa team
- Query construction is controlled by the agent through the `q` field and follows esa's post search
  syntax

## LLM Boundary

The configured LLM provider receives the Slack request, any loaded thread context, and the evidence
returned by the enabled tools so it can synthesize a report. When `search_web` is used, the query
is sent through the configured LLM provider's web-search capability to check external service
status or public incident reports.
