export interface DatadogApiRetryConfig {
	enabled: boolean;
	maxRetries: number;
	backoffBaseSeconds: number;
	backoffMultiplier: number;
}
