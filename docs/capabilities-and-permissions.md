# Capabilities and Permissions

This document lists the current runtime's tool inventory, required integration permissions, and
behavioral restrictions.

Reili is intentionally scoped around task execution and decision support. The current runtime
is investigation-focused. It can post progress and final replies in Slack, but it does not get
shell access, cluster access, or deployment credentials in production. Its effective capabilities
are the integrations and tools wired in this runtime.

## Agent Tool Inventory

The runtime can expose only the following tool families:

- Slack progress reporting: `report_progress` (primarily used to post progress messages back to Slack)
- Slack workspace lookup: `search_slack_messages` (searches prior Slack public-channel messages
  visible to the current invocation context)
- Memory when `[memory.slack]` is configured: startup recall of the channel's recent memories plus
  shared cross-channel memories from the Slack Canvas, plus two lead-only tools that persist new
  durable Fact/Evidence/Scope notes to that Canvas — `save_memory` (current channel) and
  `save_shared_memory` (shared across all channels)
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
- esa sub-agent delegation when `[connector.esa]` is configured: `esa_agent`
- JIRA MCP reads when `[connector.jira]` is configured: `searchJiraIssuesUsingJql`,
  `getJiraIssue`, `getJiraIssueRemoteIssueLinks`, `getTransitionsForJiraIssue`
- External web lookup: `search_web`

In the current runtime, no tool is registered for GitHub writes, Slack admin actions, Datadog
mutations, esa writes, JIRA writes, remediation, or deployments.

## Slack Permissions and API Usage

Reili uses Slack as both its entry point and its reporting surface.

Required Slack credentials:

- `SLACK_BOT_TOKEN`: calls Slack Web API methods
- `SLACK_SIGNING_SECRET`: verifies incoming requests on `/slack/events` in HTTP mode
- `SLACK_APP_TOKEN`: opens the Socket Mode WebSocket in Socket Mode (`xapp-...` App-Level Token)

Additional Slack platform requirement: `assistant.search.context` is available only to internal
Slack apps or apps distributed through the Slack Marketplace/App Directory. The runtime uses the
Bot Token + `action_token` flow, which supports public-channel message search through
`search:read.public`. Enabling this capability is one of the common settings below (`Agents & AI
Apps` → `Agent or Assistant`).

Required Event Subscriptions:

- Enable Events
- `app_mention`: starts a task when someone mentions the bot and carries the `action_token` used by `assistant.search.context`
- `file_shared`: fetches shared file content, including forwarded emails, and feeds it through the same auto-response path as public-channel messages
- `message.channels`: feeds public-channel posts to the auto-response judge for channels configured
  with `auto_response = true` in `[[channel.slack.channels]]`; events from other channels are
  discarded without any Slack-visible action

Configure the Slack app in two steps:

1. Apply the common settings below
2. Choose exactly one runtime mode: default `Socket Mode` or explicit `HTTP mode`

Common settings for both modes:

| Slack screen                          | Required setting                                                                                                   | Why                                                                                                              |
|---------------------------------------|--------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------|
| `Agents & AI Apps`                    | Turn on `Agent or Assistant`                                                                                       | Enables `assistant.search.context` (see platform requirement above)                                             |
| `OAuth & Permissions`                 | Add the Bot Token Scopes listed under "Bot OAuth scopes" below                                                     | Grants the permissions each capability below needs                                                              |
| `Event Subscriptions`                 | Turn on events and add the bot events `app_mention`, `file_shared`, and `message.channels`                        | See "Required Event Subscriptions" above                                                                        |
| `Interactivity & Shortcuts`           | Turn on interactivity                                                                                              | Receive `block_actions` when a user clicks a task's `Cancel` button                                              |
| `Install App` / `OAuth & Permissions` | Install or reinstall the app after any scope change                                                                | Slack does not apply updated scopes until reinstall                                                              |
| Slack workspace                       | Invite the app to every public channel where it should respond, including auto-response channels                   | The app must be present in the conversation to receive mentions, receive channel posts, and post replies         |

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

Bot OAuth scopes:

- `app_mentions:read`: receive `app_mention` events
- `chat:write`: post progress and final replies into the originating thread, post/update the task control message, and post authorization deny notices as ephemeral messages
- `reactions:write`: add an `:eyes:` reaction to the triggering message once the task is queued
- `channels:history`: read public channel thread history when additional context is needed, and receive `message.channels` events for auto-response channels
- `channels:read`: resolve public channel metadata for mention and auto-response authorization
- `files:read`: receive and inspect Slack file share events, including forwarded emails represented as files
- `usergroups:read`: resolve user group membership for mention and auto-response authorization
- `search:read.public`: call `assistant.search.context` for public-channel Slack message search with a Bot Token
- `canvases:read` (only when `[memory.slack]` is configured): read the shared memory Canvas via
  `files.info` and `canvases.sections.lookup`
- `canvases:write` (only when `[memory.slack]` is configured): append and prune memory entries in the
  shared Canvas via `canvases.edit`

Required App-Level Token scope for Socket Mode:

- `connections:write`: call `apps.connections.open` to obtain the temporary WebSocket URL

The memory Canvas must also be shared explicitly, because canvases default to "only invited people
can access". In the canvas share settings, grant Reili (the bot) both **read and write (can edit)**
access, and grant access to the team members who need to read or curate the stored memories.

Slack restrictions:

- Public channels only: direct messages, group direct messages, and private channels — including
  their related event subscriptions and history scopes — are entirely out of scope; `groups:read`
  is intentionally not requested, so private channel metadata is never read, and `app_mention` /
  `message.channels` events from private channels are denied before enqueue
- Handles only `app_mention` events from channels matching a `mention = true` entry in
  `[[channel.slack.channels]]`, combined with the user ID, user group, or bot actor authorization settings
- Evaluates `message.channels` events only for channels matching an `auto_response = true` entry;
  everything else is discarded silently, and judge declines produce no Slack-visible action
- Reads only the thread where the request was made, and only when additional thread context is needed
- Searches only Slack public-channel messages permitted by the current app install, bot token scope, and `action_token` context
- Loads channel memory only from the configured shared Slack Canvas (`[memory.slack]`), reading only
  the current channel's section; memory is disabled entirely when no Canvas is configured
- Posts only into the originating thread
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

Datadog capabilities currently used by the runtime:

- Connect to the remote Datadog MCP Server over Streamable HTTP
- Search services, logs, metrics, monitors, and events through Datadog-provided MCP tools
- Search dashboards, retrieve full dashboard definitions, and query Synthetic tests through
  Datadog-provided MCP tools
- Search security signals and investigate security findings when Datadog returns those read tools
- Fetch metric detail and context from Datadog MCP for task pivots

Datadog MCP endpoint currently used by the runtime:

- `https://mcp.<DATADOG_SITE>/api/unstable/mcp-server/mcp?toolsets=core,security,dashboards,synthetics`

This `toolsets` value is fixed internally and is not a runtime config setting. Datadog still
decides which tools it actually returns for this request, based on your plan, org configuration,
and application key permissions.

Recommended Datadog access policy:

- Create a dedicated Datadog service account for Reili, and issue its API key and application key
  rather than reusing human operator credentials
- Prefer restricted application keys when your Datadog plan supports them
- Allow only the minimum Datadog product access Reili needs across the allowlisted `core`,
  `security`, `dashboards`, and `synthetics` read tools
- Reili exposes only the intersection of its allowlisted Datadog tools and the tools Datadog
  actually returns for your credentials; if Datadog omits an allowlisted tool, Reili logs a warning
  and continues with the remaining available tools

Datadog restrictions:

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

GitHub restrictions:

- The GitHub sub-agent only receives this allowlisted subset of MCP tools:
  `search_code`, `search_repositories`, `search_issues`, `search_pull_requests`,
  `get_file_contents`, `pull_request_read`, `actions_get`, `actions_list`, `get_job_logs`,
  `get_dependabot_alert`, and `list_dependabot_alerts`
- Raw MCP write tools are not exposed to the GitHub agent in this runtime: no commenting, merging,
  labeling, reviewing, or workflow dispatch is performed
- It does not mint or refresh GitHub MCP tokens; any short-lived token rotation must happen outside the runtime
- All GitHub search and reads are scoped to `GITHUB_SEARCH_SCOPE_ORG`: Reili enforces it before every
  allowlisted call, so queries must stay inside it and repository content, pull request, Actions,
  and Dependabot reads are rejected when the owner is outside it

## esa Permissions and Scope

Reili can optionally search an esa team's posts as an internal documentation source.
This integration is disabled unless `[connector.esa]` is present in `reili.toml`. When the section
is omitted, Reili does not read `ESA_ACCESS_TOKEN` and does not register `esa_agent` or `search_posts`.

Required esa credential when configured:

- `ESA_ACCESS_TOKEN`: an esa access token with the `read` scope for the configured team

Required `reili.toml` fields when configured:

- `connector.esa.team_name`: esa team name, e.g. `docs` for `https://docs.esa.io/`
- `connector.esa.access_token_env`: optional env var name; defaults to `ESA_ACCESS_TOKEN`

esa API endpoint currently used by the runtime:

- `GET /v1/teams/:team_name/posts`

esa capabilities currently used by the runtime:

- Delegate documentation investigation to the `esa_agent` sub-agent
- Search posts through the sub-agent's `search_posts` tool using esa's `q` search syntax
- Return post metadata, links, tags, authors, pagination metadata, and Markdown body content from
  the search response

esa restrictions:

- No esa write endpoints are called by this runtime
- Reili does not create, edit, delete, star, watch, archive, share, or comment on esa posts
- Requests are scoped to the single configured esa team
- Query construction is controlled by the agent through the `q` field and follows esa's post search
  syntax

## JIRA Permissions and Scope

Reili can optionally search and reference JIRA tickets through the Atlassian Rovo MCP server.
This integration is disabled unless `[connector.jira]` is present in `reili.toml`. When the section
is omitted, Reili does not read `JIRA_SERVICE_ACCOUNT_API_TOKEN` and does not register any JIRA tools.

Runtime authentication model:

- Reili sends `JIRA_SERVICE_ACCOUNT_API_TOKEN` as a static `Authorization: Bearer` header to the
  Atlassian Rovo MCP server (`https://mcp.atlassian.com/v1/mcp`)
- This requires an Atlassian org admin to enable "Authentication via API token" for the Rovo MCP
  server (Atlassian Administration → Rovo → Rovo MCP server → Authentication), so no interactive
  OAuth consent flow is needed

Required API token scope for the service account:

The Rovo MCP server requires a *scoped* API token — a classic, unscoped API token does not carry
the scope information MCP needs, even though the underlying Jira permissions would still apply.
When an org admin creates the token for the service account
(`https://id.atlassian.com/manage-profile/security/api-tokens` → **Create API token with scopes**,
or the equivalent flow in Atlassian Administration for a managed service account), use:

- App: **Jira**
- Scope catalog: **Classic scopes** (Atlassian recommends classic scopes over granular scopes
  where a classic scope covers the need)
- Scopes: `read:jira-work`, `read:jira-user`, `read:account`, `read:me`

`read:jira-work` covers issue read, JQL search, and comments — everything the runtime's allowlisted
tools actually call. The other three scopes are required by the Rovo MCP server itself for
identity/account resolution during the connection handshake, even though no allowlisted tool
performs a user lookup; omitting any of these four scopes causes the Rovo MCP connection to fail.
Do not grant `write:jira-work` or any Confluence/Bitbucket/Jira Service Management/Compass scope —
even if the service account token is ever granted broader access by mistake, Reili itself never
requests a write tool or a non-Jira tool.

Required Jira project access for the service account:

The API token scope only controls what the Rovo MCP server may request; it does not grant issue
visibility by itself. A Jira project admin must also grant the service account the `Browse
Projects` permission in each project's permission scheme, or reads for that project return empty
results.

Required `reili.toml` fields when configured:

- `connector.jira.site`: Atlassian Cloud site hostname, e.g. `acme.atlassian.net`
- `connector.jira.service_account_api_token_env`: optional env var name; defaults to
  `JIRA_SERVICE_ACCOUNT_API_TOKEN`

JIRA capabilities currently used by the runtime:

- Search issues using a JQL query
- Read an issue's summary, description, status, assignee, comments, and issue links
- List remote links (e.g. Confluence pages, external URLs) attached to an issue
- List available workflow transitions and status options for an issue

JIRA restrictions:

- The JIRA sub-agent only receives this allowlisted subset of MCP tools:
  `searchJiraIssuesUsingJql`, `getJiraIssue`, `getJiraIssueRemoteIssueLinks`, and
  `getTransitionsForJiraIssue`
- Raw MCP write tools (`createJiraIssue`, `editJiraIssue`, `transitionJiraIssue`,
  `addCommentToJiraIssue`, `addWorklogToJiraIssue`) are not exposed to the JIRA agent in this
  runtime: no issue creation, editing, commenting, worklog changes, or workflow transitions are performed
- The configured `site` is stamped onto every call as the Atlassian `cloudId` argument, so a
  sub-agent cannot target a different Atlassian site than the one configured

## LLM Data Exposure

The configured LLM provider receives the Slack request, any loaded thread context, and the evidence
returned by the enabled tools so it can synthesize a report. When `search_web` is used, the query
is sent through the configured LLM provider's web-search capability to check external service
status or public incident reports.
