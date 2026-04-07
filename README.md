<div align="center">
  <h1>Reili</h1>
  <img src="./reili.png" alt="Reili logo" width="240" />
  <p><strong>An AI team member for SRE and DevOps, currently focused on investigations</strong></p>
  <p>
    Investigate alerts quickly from Slack with Datadog telemetry and GitHub context.
  </p>
</div>

## What is Reili?

Reili is an AI team member for your SRE and DevOps team.

Give Reili a task — it investigates by calling Datadog's read-only MCP
tools, checking recent changes on GitHub, searching relevant Slack
messages when the current thread is not enough, and reporting back with
what it found. Today Reili handles investigation; over time it will grow
into more of the responsibilities your team carries.

## Why Reili

SRE, DevOps, and on-call engineers spend much of their time on alert
response — checking dashboards, reading diffs, and deciding whether
action is needed. Reili takes that work off your plate.

Give Reili a Slack message or a task, and it will:

- Investigate Datadog logs, metrics, and events through Datadog MCP
- Explore GitHub repositories, PRs, issues, and code — connecting that
  context with Datadog to understand system structure and trace issues
- Search relevant Slack public-channel history from the current Slack
  invocation context when prior discussion matters
- Report back with what it found so you can decide what to do next

Its current focus is triage, investigation, and communicating findings.

## Core Features

- **Investigation-focused**: Reili reads and reports — it never changes your infrastructure
- **Stateless**: no database, no persistent memory — starts fresh every time
- **Chat-based**: currently works in Slack

## Quick Start

### 1. Prerequisites

- Slack App
  - Create and install it from [slack-app-manifest.yml](./slack-app-manifest.yml) or 
  <a href="https://api.slack.com/apps?new_app=1&amp;manifest_yaml=display_information%3A%0D%0A++name%3A+Reili%0D%0Afeatures%3A%0D%0A++bot_user%3A%0D%0A++++display_name%3A+Reili%0D%0A++++always_online%3A+true%0D%0Aoauth_config%3A%0D%0A++scopes%3A%0D%0A++++bot%3A%0D%0A++++++-+reactions%3Awrite%0D%0A++++++-+app_mentions%3Aread%0D%0A++++++-+channels%3Ahistory%0D%0A++++++-+channels%3Aread%0D%0A++++++-+chat%3Awrite%0D%0A++pkce_enabled%3A+false%0D%0Asettings%3A%0D%0A++event_subscriptions%3A%0D%0A++++request_url%3A+https%3A%2F%2Fexample.com%2Fslack%2Fevents%0D%0A++++bot_events%3A%0D%0A++++++-+app_mention%0D%0A++interactivity%3A%0D%0A++++is_enabled%3A+true%0D%0A++org_deploy_enabled%3A+false%0D%0A++socket_mode_enabled%3A+true%0D%0A++token_rotation_enabled%3A+false%0D%0A">Create App from manifest link</a>
  - In Slack App settings, open `Agents & AI Apps` and turn on `Agent or Assistant` so Bot Token based Slack search is available
  - Configure the required scopes, events, and Interactivity using
    [Slack Permissions and API Usage](./docs/permissions-and-boundaries.md#slack-permissions-and-api-usage).
- Datadog API Key + APP Key for the Datadog MCP server
- OpenAI API Key, AWS credentials with permission to use Amazon Bedrock, or Google Cloud ADC with permission to call
  Vertex AI Gemini models
- GitHub MCP token with read access to the repositories Reili investigates

### 2. Configure Environment Variables

Use [`.env.example`](./.env.example) as a starting point and copy it to `.env`:

```bash
cp .env.example .env
```

Then fill in the required values below.

- Collect `SLACK_BOT_TOKEN`, plus `SLACK_APP_TOKEN` for the default Socket Mode or
  `SLACK_SIGNING_SECRET`

Required:

- `SLACK_BOT_TOKEN`
- `SLACK_APP_TOKEN` when `SLACK_SOCKET_MODE` is unset or `true` (default Socket Mode)
- `SLACK_SIGNING_SECRET` when `SLACK_SOCKET_MODE=false` (HTTP mode)
- `DATADOG_API_KEY`
- `DATADOG_APP_KEY`
- `LLM_PROVIDER`
- `LLM_OPENAI_API_KEY` when `LLM_PROVIDER=openai`
- `LLM_ANTHROPIC_API_KEY` when `LLM_PROVIDER=anthropic`
- `LLM_ANTHROPIC_MODEL` when `LLM_PROVIDER=anthropic`
- `LLM_BEDROCK_MODEL_ID` when `LLM_PROVIDER=bedrock`
- `LLM_VERTEX_AI_MODEL_ID` when `LLM_PROVIDER=vertexai`
- `GOOGLE_CLOUD_LOCATION` when `LLM_PROVIDER=vertexai`
- `GOOGLE_CLOUD_PROJECT` when `LLM_PROVIDER=vertexai`
- `GITHUB_SEARCH_SCOPE_ORG`

Common optional variables:

- `PORT` (default: `3000`)
- `DATADOG_SITE` (default: `datadoghq.com`)
- `LANGUAGE` (default: `English`)
- `GITHUB_MCP_URL` (default: `https://api.githubcopilot.com/mcp/`)

GitHub configuration:

- `GITHUB_APP_ID`
- `GITHUB_APP_PRIVATE_KEY`
- `GITHUB_APP_INSTALLATION_ID`

The GitHub integration talks to a streamable HTTP MCP server and exposes a small allowlisted set
of raw GitHub MCP read tools to the GitHub specialist agent. Reili mints short-lived GitHub App
installation tokens at runtime and uses them as the MCP bearer token, so `GITHUB_MCP_TOKEN` is not
used.

`SLACK_APP_TOKEN` must be a Slack App-Level Token that starts with `xapp-`. When
`SLACK_SOCKET_MODE` is unset, Reili starts in Socket Mode. In Socket Mode, `SLACK_SIGNING_SECRET`
is not used.

When `LLM_PROVIDER=anthropic`, Claude is called through the Anthropic API.

- Set `LLM_ANTHROPIC_API_KEY` and `LLM_ANTHROPIC_MODEL`.
- Supported `LLM_ANTHROPIC_MODEL` values are `claude-opus-4-6`, `claude-sonnet-4-6`, and
  `claude-haiku-4-5`.
- `search_web` uses Anthropic's web search server tool. Your Anthropic organization administrator
  must enable web search in Claude Console, or the tool will return a soft error payload instead of
  live search results.

When `LLM_PROVIDER=bedrock`, AWS credentials and region are loaded from the standard AWS SDK environment or profile
chain. Set `AWS_PROFILE` to use a named AWS profile such as an AWS SSO profile, and set `AWS_REGION` if the selected
profile does not already define a region.
- Web search is currently unavailable with the Bedrock provider. If Reili issues a web search while
  `LLM_PROVIDER=bedrock`, it returns a `capability_unavailable` result instead of live search results.

When `LLM_PROVIDER=vertexai`, Google credentials are loaded from Application Default Credentials.

- Set `GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION`, and `LLM_VERTEX_AI_MODEL_ID`.
- For Gemini on Vertex AI, `GOOGLE_CLOUD_LOCATION=global` is usually the best default.
- Web search uses Vertex AI Gemini Grounding with Google Search.
- If Vertex AI returns `RESOURCE_EXHAUSTED`, verify your project quotas in Google Cloud Quotas (
  `https://console.cloud.google.com/iam-admin/quotas`) and adjust them if needed.

### 3. Run Reili

To get Reili running quickly in a local environment, copy [`.env.example`](./.env.example) to `.env`, fill in your
values, and start it with Docker:

```bash
docker run --rm --env-file .env ghcr.io/reilidev/reili:latest
```

If you are using HTTP mode, publish the application port as well:

```bash
docker run --rm --env-file .env -p 3000:3000 ghcr.io/reilidev/reili:latest
```

For HTTP mode, Slack must be able to reach both `/slack/events` and `/slack/interactions`. In
local development, use a public tunnel such as `ngrok` or `Cloudflare Tunnel` and set both the
Slack Event Subscriptions Request URL and the Interactivity Request URL to that public HTTPS URL.

## Usage

Mention the bot in Slack with a task request:

```text
@Reili Please investigate this alert.
```

What happens:

1. It posts a task control message with a `Cancel` button in the thread
2. It posts task progress in the thread
3. It investigates across Datadog and GitHub
4. It replies with an evidence-backed summary

If you need to stop a queued or running investigation, click `Cancel` on that task's control
message in the same Slack thread.

## Permissions and Tool Transparency

Reili is intentionally scoped around task execution and decision support. The current runtime is
investigation-focused. It can post progress and final replies in Slack, but it does not get shell
access, cluster access, or deployment credentials in production.

At a high level, the current runtime:

- reads from Datadog, GitHub, Slack thread history, Slack public-channel search, and web lookup
  integrations, and writes only Slack progress and result messages
- does not register tools for Datadog mutations, GitHub writes, remediation, or deployments
- is designed to investigate and report, not to change infrastructure, Datadog state, or repository
  state

For the full tool inventory, required Slack scopes, Datadog RBAC permissions, GitHub backend
permissions, and LLM data boundary, see
[docs/permissions-and-boundaries.md](./docs/permissions-and-boundaries.md).

## Development

For local development setup, architecture rules, and contributor workflows, see [DEVELOPERS.md](./DEVELOPERS.md).

## Release

Releases are managed with `tagpr` using Git tags and changelog updates; Cargo manifest versions are
not part of the release flow.

## Non-Goals

- Executing operational actions like auto-remediation or auto-deploy
- Heavy stateful workflow orchestration

This project is intentionally focused on investigation-oriented task execution and decision support.
