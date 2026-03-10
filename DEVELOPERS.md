# DEVELOPERS

This document is for contributors and maintainers of `sre-mate`.

## Prerequisites

- Node.js 20+
- pnpm 10+
- Slack App credentials
- Datadog API credentials
- OpenAI API key
- GitHub App credentials

## Setup

1. Install dependencies:

```bash
pnpm install
```

2. Create local environment file:

```bash
cp .env.example .env
```

3. Fill required variables in `.env`:

- Shared:
  - `SLACK_BOT_TOKEN`
  - `SLACK_SIGNING_SECRET`
  - `WORKER_INTERNAL_TOKEN`
- Ingress:
  - `WORKER_BASE_URL`
- Worker:
  - `DATADOG_API_KEY`
  - `DATADOG_APP_KEY`
  - `OPENAI_API_KEY`
  - `GITHUB_APP_ID`
  - `GITHUB_APP_PRIVATE_KEY`
  - `GITHUB_APP_INSTALLATION_ID`
  - `GITHUB_SEARCH_SCOPE_ORG`

Optional:

- `PORT` (default: `3000`)
- `WORKER_INTERNAL_PORT` (default: `3100`)
- `DATADOG_SITE` (default: `datadoghq.com`)
- `LANGUAGE` (default: `English`)

## Local Run

Run two processes in separate terminals.

Terminal 1 (worker):

```bash
bash -lc 'set -a; source ../.env; set +a; cargo watch -x "run -p sre_runtime -- --mode worker"'
```

Terminal 2 (ingress):

```bash
bash -lc 'set -a; source ../.env; set +a; cargo watch -x "run -p sre_runtime -- --mode ingress"'
```

Production-like start:

```bash
pnpm start:worker
pnpm start:ingress
```

## Validation Commands

Run these before opening a PR:

```bash
pnpm test
pnpm format
pnpm lint:deps
pnpm typecheck
```

## Architecture Rules

Project layers:

- `src/runtime`: bootstrap and runtime entrypoints
- `src/application`: use case orchestration
- `src/ports`: boundary contracts
- `src/adapters`: concrete integrations
- `src/shared`: cross-cutting types and utilities

Dependency direction:

- `application -> ports`
- `adapters -> ports`
- `runtime -> application + adapters + config`
- Avoid `application -> adapters` direct imports.

## Behavior Notes

- Ingress receives Slack events and dispatches jobs to Worker via `POST /internal/jobs`.
- Job queue is in-memory (`InMemoryJobQueue`), so pending jobs are not durable across worker restarts.

## Testing Conventions

- Add tests alongside implementation files (`*.test.ts`).
- Prefer unit tests for `application` layer orchestration and adapter contract behavior.
- Keep tests deterministic and independent from external APIs.

## Useful Scripts

- `pnpm dev:worker`: run worker with reload
- `pnpm dev:ingress`: run ingress with reload
- `pnpm dev:test`: run Vitest in watch mode
- `pnpm build`: TypeScript build
- `pnpm test`: run tests once
- `pnpm typecheck`: type check without emit
