# DEVELOPERS

This document is for contributors and maintainers of `Reili`.

## Prerequisites

- Rust stable toolchain
- Slack App credentials
- Datadog API and application keys for the Datadog MCP server
- GitHub App credentials for the repositories Reili investigates
- At least one supported AI backend:
  - OpenAI API key
  - Anthropic API key
  - AWS credentials for Amazon Bedrock
  - Google Application Default Credentials for Vertex AI

## Repository Layout

Rust code lives in the `crates/` workspace:

- `crates/core`: domain types, errors, and trait-based ports
- `crates/application`: use cases and task orchestration
- `crates/adapters`: inbound and outbound integration adapters
- `crates/runtime`: config loading, dependency wiring, HTTP entrypoint, and Slack Socket Mode entrypoint

Top-level runtime assets:

- `config/default.toml`: full runtime config template
- `config/minimum.toml`: minimal runtime config example
- `.env.example`: secret environment variable template
- `slack-app-manifest.yml`: Slack App manifest
- `docs/permissions-and-boundaries.md`: current integration permissions and runtime boundaries

## Setup

1. Prepare a local runtime config:

```bash
cp config/default.toml reili.toml
```

2. Edit `reili.toml` for local development.

Non-secret settings live in `reili.toml`, including:

- server port
- conversation language and optional additional system prompt
- Slack connection mode
- selected AI backend and backend-specific non-secret settings
- Datadog site
- GitHub MCP URL, GitHub App ID, installation ID, and search scope org

3. Prepare local secrets:

```bash
cp .env.example .env
```

4. Fill the secret values referenced by `reili.toml`.

Common required secrets:

- `SLACK_BOT_TOKEN`
- `DATADOG_API_KEY`
- `DATADOG_APP_KEY`
- `GITHUB_APP_PRIVATE_KEY`

Slack mode-specific secrets:

- `SLACK_APP_TOKEN` when `channel.slack.socket_mode = true`
- `SLACK_SIGNING_SECRET` when `channel.slack.socket_mode = false`

AI backend-specific secrets:

- `LLM_OPENAI_API_KEY` when the selected backend uses `provider = "openai"`
- `LLM_ANTHROPIC_API_KEY` when the selected backend uses `provider = "anthropic"`
- Bedrock credentials are loaded from the standard AWS SDK environment/profile chain
- Vertex AI credentials are loaded from Google Application Default Credentials

`GITHUB_MCP_TOKEN` is not used. Reili signs a GitHub App JWT with `GITHUB_APP_PRIVATE_KEY`,
exchanges it for a short-lived installation token, and uses that token as the GitHub MCP bearer
token at runtime.

## Runtime Config

The runtime loads config from:

1. `--config /path/to/reili.toml`
2. `./reili.toml`

If the selected config file does not exist, startup fails.

Supported config schema version:

```toml
version = 1
```

The current AI backend selector is `ai.default_backend`, which points to one entry under
`ai.backends`. Do not use the old `LLM_PROVIDER` environment variable pattern.

Supported backend providers:

- `openai`
- `anthropic`
- `bedrock`
- `vertexai` (`vertex_ai` is also accepted by the loader)

OpenAI backends require `model` and may set `reasoning_effort` to `low`, `medium`, `high`, or
`xhigh`. Anthropic backends currently accept `claude-opus-4-6`, `claude-sonnet-4-6`, and
`claude-haiku-4-5`. Bedrock and Vertex AI backends use `model_id`.

## Local Run

Run from the repository root and pass the root `reili.toml` explicitly:

```bash
bash -lc 'set -a; source .env; set +a; cargo run --manifest-path crates/Cargo.toml -p reili_runtime -- --config reili.toml' 2>&1 | tee .tmp/reili.log
```

If you prefer running from `crates/`:

```bash
cd crates
bash -lc 'set -a; source ../.env; set +a; cargo run -p reili_runtime -- --config ../reili.toml' 2>&1 | tee ../.tmp/reili.log
```

If you use `cargo-watch`:

```bash
cd crates
bash -lc 'set -a; source ../.env; set +a; cargo watch -x "run -p reili_runtime -- --config ../reili.toml"' 2>&1 | tee ../.tmp/reili.log
```

## Slack Runtime Modes

Socket Mode is the default:

```toml
[channel.slack]
socket_mode = true
```

Socket Mode requires `SLACK_BOT_TOKEN` and `SLACK_APP_TOKEN`. The app token must be an App-Level
Token that starts with `xapp-`.

HTTP mode:

```toml
[channel.slack]
socket_mode = false
```

HTTP mode requires `SLACK_BOT_TOKEN` and `SLACK_SIGNING_SECRET`. The runtime exposes:

- `POST /slack/events`
- `POST /slack/interactions`
- `GET /healthz`

For local HTTP mode testing, expose the app with a public HTTPS tunnel and configure both Slack
Event Subscriptions and Interactivity to use that public URL.

## Docker

For normal local development, use `cargo run`. The checked-in `Dockerfile` is the release image
packaging step: it expects a prebuilt `dist/docker/<arch>/reili` binary prepared by the release
workflow.

To run the published image, provide both `.env` and `reili.toml` as described in the README:

```bash
docker run --rm \
  --env-file .env \
  -v "$(pwd)/reili.toml:/home/reili/reili.toml:ro" \
  ghcr.io/reilidev/reili:latest
```

For HTTP mode, publish the app port:

```bash
docker run --rm \
  --env-file .env \
  -v "$(pwd)/reili.toml:/home/reili/reili.toml:ro" \
  -p 3000:3000 \
  ghcr.io/reilidev/reili:latest
```

## Validation Commands

Run these before opening a PR that changes Rust code:

```bash
cd crates
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace --locked --all-targets`.

## Architecture Rules

Dependency direction:

- `application -> core`
- `adapters -> core`
- `runtime -> application + adapters + core`
- Avoid `application -> adapters` direct imports
- Keep `core` independent from runtime and adapter concerns

Use trait-based ports and constructor injection for external dependencies. Ports currently live in
their domain modules under `crates/core/src`, for example `messaging/slack`, `knowledge`, `queue`,
`source_code/github`, and `task`.

When adding behavior:

- Put domain data and boundary traits in `core`
- Put orchestration in `application`
- Put concrete external integrations in `adapters`
- Wire concrete implementations in `runtime/bootstrap`
- Prefer explicit input structs when a function or constructor needs several inputs
- Avoid `unwrap` and `expect` in production code

## Runtime Behavior Notes

- Slack `app_mention` events enqueue task jobs into the in-memory queue.
- The same runtime process starts worker tasks and claims jobs from that queue.
- Jobs are not durable across app restarts.
- Reili posts a task control message with a `Cancel` button, streams progress in the originating
  Slack thread, and posts a final reply.
- Cancel interactions are handled through Slack Interactivity in both Socket Mode and HTTP mode.
- The current runtime is investigation-focused. It reads Datadog, GitHub, Slack thread history,
  Slack public-channel search, and web lookup integrations, and writes only Slack progress/control
  and result messages.

## Testing Conventions

- Add Rust tests alongside implementation code with `#[cfg(test)]` or sibling test modules.
- Prefer unit tests for application orchestration, config resolution, and adapter contract behavior.
- Keep tests deterministic and independent from external APIs.
- Do not add tests that only verify logging.

## Release Notes

Releases are managed with `tagpr` using Git tags and changelog updates. Cargo manifest versions are
kept at the workspace placeholder version and are not part of the release flow.

Release binaries are built by `.github/workflows/_build-release-bundle.yml`. Container images are
assembled from those prebuilt binaries and published by the release workflow.

## Useful Commands

- `cargo run -p reili_runtime -- --config ../reili.toml`: start the app from `crates/`
- `cargo watch -x "run -p reili_runtime -- --config ../reili.toml"`: start with reload from `crates/`
- `cargo test --workspace`: run Rust tests
- `cargo clippy --workspace --all-targets -- -D warnings`: run lints
