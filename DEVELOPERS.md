# DEVELOPERS

This document is for contributors and maintainers of `Reili`.

## Prerequisites

- Rust stable toolchain
- Slack App credentials
- Datadog API credentials
- OpenAI API key
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
- `LLM_BEDROCK_REGION` when `LLM_PROVIDER=bedrock`
- `LLM_BEDROCK_MODEL_ID` when `LLM_PROVIDER=bedrock`
- `GITHUB_APP_ID`
- `GITHUB_APP_PRIVATE_KEY`
- `GITHUB_APP_INSTALLATION_ID`
- `GITHUB_SEARCH_SCOPE_ORG`

Optional:

- `PORT` (default: `3000`)
- `DATADOG_SITE` (default: `datadoghq.com`)
- `LANGUAGE` (default: `English`)

When `LLM_PROVIDER=bedrock`, AWS credentials are loaded from the standard AWS SDK environment or profile chain.

## Local Run

Run the unified runtime in one terminal.

```bash
cd crates
bash -lc 'set -a; source ../.env; set +a; cargo run -p reili_runtime'
```

If you use `cargo-watch`:

```bash
cd crates
bash -lc 'set -a; source ../.env; set +a; cargo watch -x "run -p reili_runtime"'
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
