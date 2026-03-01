import type { DatadogEventSearchPort } from "./datadog-event-search";
import type { DatadogLogAggregatePort } from "./datadog-log-aggregate";
import type { DatadogLogSearchPort } from "./datadog-log-search";
import type { DatadogMetricCatalogPort } from "./datadog-metric-catalog";
import type { DatadogMetricQueryPort } from "./datadog-metric-query";
import type { GithubSearchPort } from "./github-search";

export interface InvestigationResources {
	logAggregatePort: DatadogLogAggregatePort;
	logSearchPort: DatadogLogSearchPort;
	metricCatalogPort: DatadogMetricCatalogPort;
	metricQueryPort: DatadogMetricQueryPort;
	eventSearchPort: DatadogEventSearchPort;
	datadogSite: string;
	githubScopeOrg: string;
	githubSearchPort: GithubSearchPort;
}

export interface InvestigationRuntime {
	startedAtIso: string;
	channel: string;
	threadTs: string;
	retryCount: number;
}

export interface InvestigationContext {
	resources: InvestigationResources;
	runtime: InvestigationRuntime;
}
