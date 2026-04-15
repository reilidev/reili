<div align="center">
  <h1>Reili</h1>
  <img src="./reili.png" alt="Reili logo" width="240" />
  <p><strong>An AI team member for SRE and DevOps, currently focused on investigations</strong></p>
  <p>
    Investigate alerts quickly from Slack with Datadog telemetry and GitHub context.
  </p>
</div>

## What is Reili?
Reili works as an AI team member on your team, handling SRE and DevOps responsibilities.

When you assign a task to Reili, it will gather information from sources such as Datadog, GitHub, and Slack to carry out the work
As a general rule, Reili does not make changes to the production environment or perform recovery operations; instead,
it uses the gathered information to investigate issues and generate reports.

## Why Reili

SRE, DevOps, and on-call engineers spend much of their time on alert
response — checking dashboards, reading diffs, and deciding whether
action is needed. Reili takes that work off your plate.

Give Reili a Slack message or a task, and it will:

- Investigate in Slack public channels like a teammate, working from the
  ongoing conversation where your team is already collaborating
- Connect Datadog telemetry, GitHub repositories and changes, and
  relevant Slack public-channel history to build investigation context
- Report back with what it found so your team can decide what to do next
- Expand over time to cover additional external services beyond Datadog, GitHub, and Slack

## Core Features

- **Investigation-focused**: Reili reads and reports — it never changes your infrastructure
- **Cross-service**: works across Datadog, GitHub, and Slack today, with additional services planned over time
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
- GitHub App credentials for the repositories Reili investigates
  - Create and install it from [pages/create-github-app.html](./pages/create-github-app.html)
  - Configure the required permissions and scope in
    [GitHub Permissions and Scope](./docs/permissions-and-boundaries.md#github-permissions-and-scope).

### 2. Configure `reili.toml`

Use [`config/default.toml`](./config/default.toml) as the checked-in template for runtime settings:

```bash
cp config/default.toml reili.toml
```

For a stripped-down example that relies on defaults, see [`config/minimum.toml`](./config/minimum.toml).

Then edit `reili.toml` for your environment.

Non-secret settings live in `reili.toml`, including:

- server port
- conversation language
- Slack connection mode
- selected AI backend and backend-specific non-secret settings
- Datadog site
- GitHub MCP URL, GitHub App ID, installation ID, and search scope org

Runtime config resolution is:

1. `--config /path/to/reili.toml`
2. `./reili.toml`

If neither path exists, startup fails.

### 3. Configure Secrets

Use [`.env.example`](./.env.example) as a starting point and copy it to `.env`:

```bash
cp .env.example .env
```

Then fill in the secret values referenced by `reili.toml`.

Required secrets depend on your selected Slack mode and backend:

- `SLACK_BOT_TOKEN`
- `SLACK_APP_TOKEN` when `channel.slack.socket_mode = true`
- `SLACK_SIGNING_SECRET` when `channel.slack.socket_mode = false`
- `DATADOG_API_KEY`
- `DATADOG_APP_KEY`
- `GITHUB_APP_PRIVATE_KEY`
- `LLM_OPENAI_API_KEY` when the selected backend uses `provider = "openai"`
- `LLM_ANTHROPIC_API_KEY` when the selected backend uses `provider = "anthropic"`

The GitHub integration talks to a streamable HTTP MCP server and exposes a small allowlisted set
of raw GitHub MCP read tools to the GitHub specialist agent. Reili mints short-lived GitHub App
installation tokens at runtime and uses them as the MCP bearer token, so `GITHUB_MCP_TOKEN` is not
used. GitHub App ID, installation ID, scope org, and MCP URL are configured in `reili.toml`.

`SLACK_APP_TOKEN` must be a Slack App-Level Token that starts with `xapp-`.

When the selected backend uses `provider = "anthropic"`, Claude is called through the Anthropic
API.

- Set `api_key_env = "LLM_ANTHROPIC_API_KEY"` on that backend in `reili.toml`.
- Supported Anthropic model values are `claude-opus-4-6`, `claude-sonnet-4-6`, and
  `claude-haiku-4-5`.
- `search_web` uses Anthropic's web search server tool. Your Anthropic organization administrator
  must enable web search in Claude Console, or the tool will return a soft error payload instead of
  live search results.

When the selected backend uses `provider = "bedrock"`, AWS credentials are loaded from the standard
AWS SDK chain. Set `aws_profile` and `aws_region` in `reili.toml` when you want to force a named
profile or region for that backend. The underlying AWS credentials still come from the normal AWS
environment or profile chain.
- Web search is currently unavailable with the Bedrock provider. If Reili issues a web search while
  the selected backend uses `provider = "bedrock"`, it returns a `capability_unavailable` result
  instead of live search results.

When the selected backend uses `provider = "vertexai"`, Google credentials are loaded from
Application Default Credentials.

- Set `project_id`, `location`, and `model_id` in `reili.toml`.
- For Gemini on Vertex AI, `location = "global"` is usually the best default.
- Web search uses Vertex AI Gemini Grounding with Google Search.
- If Vertex AI returns `RESOURCE_EXHAUSTED`, verify your project quotas in Google Cloud Quotas (
  `https://console.cloud.google.com/iam-admin/quotas`) and adjust them if needed.

### 4. Run Reili

To run Reili locally with Docker, provide both `.env` and `reili.toml`:

```bash
docker run --rm \
  --env-file .env \
  -v "$(pwd)/reili.toml:/home/reili/reili.toml:ro" \
  ghcr.io/reilidev/reili:latest
```

If you are using HTTP mode, publish the application port as well:

```bash
docker run --rm \
  --env-file .env \
  -v "$(pwd)/reili.toml:/home/reili/reili.toml:ro" \
  -p 3000:3000 \
  ghcr.io/reilidev/reili:latest
```

If you need to override discovery order explicitly, pass `--config` to the runtime:

```bash
docker run --rm \
  --env-file .env \
  -v "$(pwd)/reili.toml:/work/reili.toml:ro" \
  ghcr.io/reilidev/reili:latest \
  --config /work/reili.toml
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
