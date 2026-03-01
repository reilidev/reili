# Guidelines

## Project Purpose

The SRE AI Agent is a service that responds to Slack alerts and GitHub activity by analyzing logs, metrics.

## Technologies

- TypeScript
- package manager: pnpm
- Framework: Slack Bolt, [OpenAI Agents SDK](https://openai.github.io/openai-agents-js/)
- Use v2 client of `@datadog/datadog-api-client`

## Structure

```
src/                                # Root implementation directory for the SRE Agent
├── app/                            # Application bootstrap and runtime wiring
│   ├── config/                     # Environment/config loading and validation
│   └── server/                     # Web server startup and endpoint exposure
├── usecases/                       # Flow control for end-to-end use cases
│   └── incident-handler/           # Planning and execution of alert investigation workflow
├── capabilities/                   # Business logic split by functional capability
│   ├── alert-intake/               # Intake and normalization of Slack/Datadog alerts
│   ├── investigation/              # Analysis of logs, metrics, and GitHub activity
│   └── response/                   # Building structured cause/remediation responses
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
│       └── agents/                 # OpenAPI Agents SDK integration
└── shared/                         # Reusable cross-cutting components
    ├── types/                      # Shared domain/DTO type definitions
    ├── errors/                     # Common error types and exception mapping
    ├── observability/              # Logging, metrics, and tracing helpers
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
* When you modify the code, run `pnpm test` to format it.

## Format

When you modify the code, run `pnpm format` to format it.
