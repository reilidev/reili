# Changelog

All notable changes to this project will be documented in this file.

## [v0.1.0](https://github.com/reilidev/reili/compare/0.0.1...v0.1.0) - 2026-04-02
- refactor: refine progress stream reporting boundaries by @clover0 in https://github.com/reilidev/reili/pull/9
- chore: fix release-please config by @clover0 in https://github.com/reilidev/reili/pull/10
- efactor: unify logging under core logger by @clover0 in https://github.com/reilidev/reili/pull/11
- refactor: structure port errors across adapters by @clover0 in https://github.com/reilidev/reili/pull/12
- Enable core automock by @clover0 in https://github.com/reilidev/reili/pull/13
- test: expose core generated mocks for application tests by @clover0 in https://github.com/reilidev/reili/pull/14
- refactor: centralize github scope policy by @clover0 in https://github.com/reilidev/reili/pull/15
- refactor: replace AlertContext with InvestigationRequest by @clover0 in https://github.com/reilidev/reili/pull/16
- refactor: rename investigation domain to task by @clover0 in https://github.com/reilidev/reili/pull/17
- refactor: split datadog mcp transport from tool adapters by @clover0 in https://github.com/reilidev/reili/pull/18
- Fix datadog mcp tool progress by @clover0 in https://github.com/reilidev/reili/pull/19
- chore: migrate release management to release-plz by @clover0 in https://github.com/reilidev/reili/pull/20
- chore: switch release pipeline to release-please simple by @clover0 in https://github.com/reilidev/reili/pull/21
- use tagpr by @clover0 in https://github.com/reilidev/reili/pull/23
- enable bedrock sso profile support by @clover0 in https://github.com/reilidev/reili/pull/25
- add vertex ai claude support with web search by @clover0 in https://github.com/reilidev/reili/pull/27
- rotate slack progress streams before time limit by @clover0 in https://github.com/reilidev/reili/pull/28
- preserve active scopes across slack stream rotation by @clover0 in https://github.com/reilidev/reili/pull/29
- enable dependabot by @clover0 in https://github.com/reilidev/reili/pull/30
- add push trigger for cache by @clover0 in https://github.com/reilidev/reili/pull/35
- Bump actions/create-github-app-token from 2.2.1 to 3.0.0 in /.github/workflows by @dependabot[bot] in https://github.com/reilidev/reili/pull/31
- Bump distroless/static from `28efbe9` to `47b2d72` by @dependabot[bot] in https://github.com/reilidev/reili/pull/32
- add sanitized tool execution logging by @clover0 in https://github.com/reilidev/reili/pull/34
- add slack web-socket mode support by @clover0 in https://github.com/reilidev/reili/pull/36
- add slack reaction indicator for queued mentions by @clover0 in https://github.com/reilidev/reili/pull/37
- default slack startup to socket mode by @clover0 in https://github.com/reilidev/reili/pull/38
- update README by @clover0 in https://github.com/reilidev/reili/pull/39
- feat: add Slack task cancellation flow with per-task control messages by @clover0 in https://github.com/reilidev/reili/pull/40
- Remove groups:history scope requirement from documentation by @clover0 in https://github.com/reilidev/reili/pull/42
- cargo update by @clover0 in https://github.com/reilidev/reili/pull/41
- stop tagpr from updating cargo workspace versions by @clover0 in https://github.com/reilidev/reili/pull/43
- Release for v0.1.0 by @reilidev-bot[bot] in https://github.com/reilidev/reili/pull/26
- Add GitHub App setup page and GitHub Pages deployment by @clover0 in https://github.com/reilidev/reili/pull/45

## [v0.1.0](https://github.com/reilidev/reili/compare/0.0.1...v0.1.0) - 2026-04-02
- refactor: refine progress stream reporting boundaries by @clover0 in https://github.com/reilidev/reili/pull/9
- chore: fix release-please config by @clover0 in https://github.com/reilidev/reili/pull/10
- efactor: unify logging under core logger by @clover0 in https://github.com/reilidev/reili/pull/11
- refactor: structure port errors across adapters by @clover0 in https://github.com/reilidev/reili/pull/12
- Enable core automock by @clover0 in https://github.com/reilidev/reili/pull/13
- test: expose core generated mocks for application tests by @clover0 in https://github.com/reilidev/reili/pull/14
- refactor: centralize github scope policy by @clover0 in https://github.com/reilidev/reili/pull/15
- refactor: replace AlertContext with InvestigationRequest by @clover0 in https://github.com/reilidev/reili/pull/16
- refactor: rename investigation domain to task by @clover0 in https://github.com/reilidev/reili/pull/17
- refactor: split datadog mcp transport from tool adapters by @clover0 in https://github.com/reilidev/reili/pull/18
- Fix datadog mcp tool progress by @clover0 in https://github.com/reilidev/reili/pull/19
- chore: migrate release management to release-plz by @clover0 in https://github.com/reilidev/reili/pull/20
- chore: switch release pipeline to release-please simple by @clover0 in https://github.com/reilidev/reili/pull/21
- use tagpr by @clover0 in https://github.com/reilidev/reili/pull/23
- enable bedrock sso profile support by @clover0 in https://github.com/reilidev/reili/pull/25
- add vertex ai claude support with web search by @clover0 in https://github.com/reilidev/reili/pull/27
- rotate slack progress streams before time limit by @clover0 in https://github.com/reilidev/reili/pull/28
- preserve active scopes across slack stream rotation by @clover0 in https://github.com/reilidev/reili/pull/29
- enable dependabot by @clover0 in https://github.com/reilidev/reili/pull/30
- add push trigger for cache by @clover0 in https://github.com/reilidev/reili/pull/35
- Bump actions/create-github-app-token from 2.2.1 to 3.0.0 in /.github/workflows by @dependabot[bot] in https://github.com/reilidev/reili/pull/31
- Bump distroless/static from `28efbe9` to `47b2d72` by @dependabot[bot] in https://github.com/reilidev/reili/pull/32
- add sanitized tool execution logging by @clover0 in https://github.com/reilidev/reili/pull/34
- add slack web-socket mode support by @clover0 in https://github.com/reilidev/reili/pull/36
- add slack reaction indicator for queued mentions by @clover0 in https://github.com/reilidev/reili/pull/37
- default slack startup to socket mode by @clover0 in https://github.com/reilidev/reili/pull/38
- update README by @clover0 in https://github.com/reilidev/reili/pull/39
- feat: add Slack task cancellation flow with per-task control messages by @clover0 in https://github.com/reilidev/reili/pull/40
- Remove groups:history scope requirement from documentation by @clover0 in https://github.com/reilidev/reili/pull/42
- cargo update by @clover0 in https://github.com/reilidev/reili/pull/41
- stop tagpr from updating cargo workspace versions by @clover0 in https://github.com/reilidev/reili/pull/43

## [v0.0.1](https://github.com/reilidev/reili/compare/0.0.1...v0.0.1) - 2026-03-24
- refactor: refine progress stream reporting boundaries by @clover0 in https://github.com/reilidev/reili/pull/9
- chore: fix release-please config by @clover0 in https://github.com/reilidev/reili/pull/10
- efactor: unify logging under core logger by @clover0 in https://github.com/reilidev/reili/pull/11
- refactor: structure port errors across adapters by @clover0 in https://github.com/reilidev/reili/pull/12
- Enable core automock by @clover0 in https://github.com/reilidev/reili/pull/13
- test: expose core generated mocks for application tests by @clover0 in https://github.com/reilidev/reili/pull/14
- refactor: centralize github scope policy by @clover0 in https://github.com/reilidev/reili/pull/15
- refactor: replace AlertContext with InvestigationRequest by @clover0 in https://github.com/reilidev/reili/pull/16
- refactor: rename investigation domain to task by @clover0 in https://github.com/reilidev/reili/pull/17
- refactor: split datadog mcp transport from tool adapters by @clover0 in https://github.com/reilidev/reili/pull/18
- Fix datadog mcp tool progress by @clover0 in https://github.com/reilidev/reili/pull/19
- chore: migrate release management to release-plz by @clover0 in https://github.com/reilidev/reili/pull/20
- chore: switch release pipeline to release-please simple by @clover0 in https://github.com/reilidev/reili/pull/21
- use tagpr by @clover0 in https://github.com/reilidev/reili/pull/23
