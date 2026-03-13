# Guidelines

## Project Purpose

Reili is a service that responds to Slack alerts and GitHub activity by analyzing logs and metrics.

## Structure

```
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
