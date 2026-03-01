import { client } from "@datadog/datadog-api-client";
import type { DatadogApiRetryConfig } from "../../../shared/types/datadog-api-retry";

export interface DatadogClientConfigInput {
	apiKey: string;
	appKey: string;
	site: string;
	retry: DatadogApiRetryConfig;
}

export type DatadogClientConfig = ReturnType<typeof client.createConfiguration>;

export function createDatadogClientConfig(input: DatadogClientConfigInput) {
	const datadogConfig = client.createConfiguration({
		authMethods: {
			apiKeyAuth: input.apiKey,
			appKeyAuth: input.appKey,
		},
		enableRetry: input.retry.enabled,
		maxRetries: input.retry.maxRetries,
		backoffBase: input.retry.backoffBaseSeconds,
		backoffMultiplier: input.retry.backoffMultiplier,
	});

	datadogConfig.setServerVariables({ site: input.site });
	return datadogConfig;
}
