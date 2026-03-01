import type { RunContext } from "@openai/agents";
import type { InvestigationContext } from "../investigation-agents";

export function requireInvestigationContext(
	context: RunContext<InvestigationContext> | undefined,
): InvestigationContext {
	if (!context) {
		throw new Error("RunContext is required for investigation tools");
	}

	return context.context;
}
