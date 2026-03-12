# Guidelines

## Project Purpose

Reili is a service that responds to Slack alerts and GitHub activity by analyzing logs and metrics.

## Technologies

- TypeScript
- package manager: pnpm
- Framework: Slack Bolt, [OpenAI Agents SDK](https://openai.github.io/openai-agents-js/)
- Use v2 client of `@datadog/datadog-api-client`

## Structure

```
src/                                # Root implementation directory for Reili
├── runtime/                        # Application bootstrap and runtime wiring
│   ├── bootstrap/                  # Shared dependency construction and DI helpers
│   ├── config/                     # Environment/config loading and validation
│   ├── ingress/                    # Ingress app entrypoint (Slack Events API receiver)
│   └── worker/                     # Worker app entrypoint (job processing runtime)
├── application/                    # Application layer orchestration and workflow rules
│   ├── alert-intake/               # Alert context normalization for investigation flow
│   ├── investigation/              # Investigation execution orchestration
│   │   └── services/               # Investigation-specific application services
│   └── enqueue/start-*             # Job enqueue and worker runner orchestration
├── ports/                          # Contracts abstracting external boundaries from core logic
│   ├── inbound/                    # Input-side interfaces (event intake contracts)
│   └── outbound/                   # Output-side interfaces (external API contracts)
├── adapters/                       # Concrete implementations of ports (SDK/HTTP integration)
│   ├── inbound/                    # Converts external input into internal application events
│   │   ├── http/                   # HTTP adapter for Slack Events endpoint
│   │   └── slack/                  # Slack Bolt event handling implementation
│   └── outbound/                   # Integrations for investigation and notification delivery
│       ├── datadog/                # Datadog API client integration
│       ├── slack/                  # Slack message and thread reply integration
│       ├── github/                 # GitHub API integration
│       ├── queue/                  # Job queue adapter implementations
│       ├── worker/                 # Worker dispatch adapter implementations
│       └── agents/                 # OpenAI Agents SDK integration
└── shared/                         # Reusable cross-cutting components
    ├── types/                      # Shared domain/DTO type definitions
    ├── errors/                     # Common error types and exception mapping
    ├── observability/              # Logging, metrics, and tracing helpers
    ├── validation/                 # Shared schema validation
    └── utils/                      # Generic utility helpers
```

## Principles

* Testability: Ensure the implementation is testable.
* Implement with a strong emphasis on the SOLID principles.
* When a function/method takes three or more arguments, define a dedicated input type (e.g., an input object/DTO).
* Express intent through design, naming, and types—not through comments.
* Avoid use typeof, any type, unknown type.
* Avoid using null, object type, and undefined.

## Testing

* Write tests together with implementation changes.
* Place test files in the same directory as the implementation and name them `*.test.ts`.
* When you modify the code, run `pnpm test`.

## Format

When you modify the code, run `pnpm format` to format it.

## Linting

When you modify the code, run `pnpm lint:deps` to lint layer dependencies.

## Rust Project (`rust/`)

### Structure

```text
rust/
├── Cargo.toml                      # Workspace definition
└── crates/
    ├── shared/                     # Shared types, ports, errors
    ├── application/                # Use cases and orchestration
    ├── adapters/                   # External integrations and port implementations
    └── runtime/                    # App bootstrap and runtime wiring
```

### Principles

* Keep dependency direction strict: `runtime -> application -> shared`, `runtime -> adapters -> shared`.
* Use trait-based ports and constructor injection (`Arc<dyn Trait>`) for testability.
* Prefer explicit types and domain-focused value objects; avoid primitive obsession.
* Handle failures with typed errors (`thiserror`) and propagate with context.
* Avoid `unwrap`/`expect` in production code; handle and return errors explicitly.

### Testing

* Write tests together with implementation changes.
* Place tests in the same module/file scope using `#[cfg(test)]` or sibling `tests` modules.
* When you modify Rust code, run `cargo test --workspace` in `rust/`.

### Format

When you modify Rust code, run `cargo fmt --all` in `rust/`.

### Linting

When you modify Rust code, run `cargo clippy --workspace --all-targets -- -D warnings` in `rust/`.
