# Guidelines

## Project Purpose

Reili is a Slack-native AI agent for read-only DevOps investigations. It responds to Slack alerts and GitHub activity by analyzing Datadog telemetry, GitHub context, and Slack thread history.

## Principles

* Testability: design APIs and module boundaries so behavior can be verified with focused tests.
* Keep dependency direction strict: `runtime -> application -> core`, `runtime -> adapters -> core`.
* Use trait-based ports and constructor injection (`Arc<dyn Trait>`) for external dependencies.
* Prefer explicit, domain-focused types over primitive obsession.
* When a function or constructor needs several inputs, introduce a dedicated input type.
* Express intent through naming, types, and module boundaries rather than explanatory comments.
* Avoid `unwrap` and `expect` in production code; return typed errors with context.

## Testing

* Write tests together with implementation changes.
* Place Rust tests in the same module/file scope with `#[cfg(test)]` or sibling `tests` modules.
* Keep tests deterministic and isolated from external services.
* Do not add tests that only verify logging; prefer tests that assert observable behavior instead.
* When you modify Rust code, run `cargo test --workspace` in `crates/`.

## Format

When you modify Rust code, run `cargo fmt --all` in `crates/`.

## Linting

When you modify Rust code, run `cargo clippy --workspace --all-targets -- -D warnings` in `crates/`.
