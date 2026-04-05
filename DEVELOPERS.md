# DEVELOPERS

This document is for contributors and maintainers of `Reili`.

## Prerequisites

- Rust stable toolchain
- Slack App credentials
- Datadog API credentials for the Datadog MCP server
- OpenAI API key, AWS credentials for Bedrock, or Google Cloud ADC for Vertex AI
- GitHub App credentials

## Setup

1. Prepare local environment file:

```bash
cp .env.example .env
```

2. Fill required variables in `.env`:

- `SLACK_BOT_TOKEN`
- `SLACK_SIGNING_SECRET`
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
- `GITHUB_APP_ID`
- `GITHUB_APP_PRIVATE_KEY`
- `GITHUB_APP_INSTALLATION_ID`
- `GITHUB_SEARCH_SCOPE_ORG`

Optional:

- `PORT` (default: `3000`)
- `DATADOG_SITE` (default: `datadoghq.com`)
- `LANGUAGE` (default: `English`)

When `LLM_PROVIDER=anthropic`, set `LLM_ANTHROPIC_API_KEY` and `LLM_ANTHROPIC_MODEL`. The
supported `LLM_ANTHROPIC_MODEL` values are `claude-opus-4-6`, `claude-sonnet-4-6`, and
`claude-haiku-4-5`. The
`search_web` tool uses Anthropic's web search server tool, which must be enabled by your Anthropic
organization administrator in Claude Console.

When `LLM_PROVIDER=bedrock`, AWS credentials and region are loaded from the standard AWS SDK environment or profile chain. Set `AWS_PROFILE` to use a named AWS profile such as an AWS SSO profile, and set `AWS_REGION` if the selected profile does not already define a region.


When `LLM_PROVIDER=vertexai`, Google credentials are loaded from Application Default Credentials. Set `GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION`, and `LLM_VERTEX_AI_MODEL_ID`. Vertex AI web search uses Gemini Grounding with Google Search, so the selected model and project must have access to that capability.

## Local Run

Run the unified runtime in one terminal.

```bash
cd crates
bash -lc 'set -a; source ../.env; set +a; APP_VERSION=local cargo run -p reili_runtime' 2>&1 | tee ../.tmp/reili.log
```

If you use `cargo-watch`:

```bash
cd crates
bash -lc 'set -a; source ../.env; set +a; APP_VERSION=local cargo watch -x "run -p reili_runtime"' 2>&1 | tee ../.tmp/reili.log
```

## Docker Run

Build a local image:

```bash
docker build --build-arg APP_VERSION=local -t reili:local .
docker run --env-file .env -p 3000:3000 reili:local
```

To consume the published GitHub Container Registry image after release, set the image name in
`compose.example.yaml` and start it with Docker Compose:

```bash
docker compose -f compose.example.yaml up -d
```

## Validation Commands

Run these before opening a PR:

```bash
cd crates
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Architecture Rules

Project layers:

- `crates/runtime`: bootstrap and runtime entrypoints
- `crates/application`: use case orchestration
- `crates/core/src/ports`: boundary contracts
- `crates/adapters`: concrete integrations
- `crates/core`: cross-cutting types and utilities

Dependency direction:

- `application -> ports`
- `adapters -> ports`
- `runtime -> application + adapters + config`
- Avoid `application -> adapters` direct imports.

## Behavior Notes

- The runtime receives Slack events at `/slack/events` and enqueues task jobs directly into `InMemoryJobQueue`.
- Worker tasks run in the same process and claim jobs from that queue.
- Pending jobs are not durable across app restarts.

## Testing Conventions

- Add Rust tests alongside implementation code with `#[cfg(test)]` or sibling test modules.
- Prefer unit tests for `application` layer orchestration and adapter contract behavior.
- Keep tests deterministic and independent from external APIs.

## Useful Commands

- `cargo run -p reili_runtime`: start the app
- `cargo watch -x "run -p reili_runtime"`: start with reload if `cargo-watch` is installed
- `cargo test --workspace`: run Rust tests
