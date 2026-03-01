# sre-mate

Slack Events API の受信経路 (Ingress) と AI 調査実行経路 (Worker) を分離した SRE エージェント実装です。

## Required Environment Variables

### Ingress

- `SLACK_BOT_TOKEN`
- `SLACK_SIGNING_SECRET`
- `WORKER_BASE_URL`
- `WORKER_INTERNAL_TOKEN`
- `PORT` (optional, default: `3000`)
- `JOB_MAX_RETRY` (optional, default: `2`)
- `JOB_BACKOFF_MS` (optional, default: `1000`)
- `WORKER_DISPATCH_TIMEOUT_MS` (optional, default: `3000`)

### Worker

- `SLACK_BOT_TOKEN`
- `SLACK_SIGNING_SECRET`
- `WORKER_INTERNAL_TOKEN`
- `DATADOG_API_KEY`
- `DATADOG_APP_KEY`
- `OPENAI_API_KEY`
- `LANGUAGE` (optional, default: `English`)
- `DATADOG_SITE` (optional, default: `datadoghq.com`)
- `WORKER_INTERNAL_PORT` (optional, default: `3100`)
- `WORKER_CONCURRENCY` (optional, default: `2`)
- `JOB_MAX_RETRY` (optional, default: `2`)
- `JOB_BACKOFF_MS` (optional, default: `1000`)

`.env.example` を参照して設定します。

## Run

```bash
pnpm dev:worker
pnpm dev:ingress
```

or

```bash
pnpm start:worker
pnpm start:ingress
```
