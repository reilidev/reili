import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createLogger } from "./logger";

const FIXED_TIMESTAMP = "2025-01-02T03:04:05.000Z";

describe("createLogger", () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(new Date(FIXED_TIMESTAMP));
	});

	afterEach(() => {
		vi.restoreAllMocks();
		vi.useRealTimers();
	});

	it("writes info logs as JSON to stdout", () => {
		const stdoutSpy = vi.spyOn(console, "log").mockImplementation(() => {
			return;
		});
		const stderrSpy = vi.spyOn(console, "error").mockImplementation(() => {
			return;
		});
		const logger = createLogger();

		logger.info("investigation started", { jobId: "job-1" });

		expect(stdoutSpy).toHaveBeenCalledWith(
			JSON.stringify({
				level: "INFO",
				message: "investigation started",
				timestamp: FIXED_TIMESTAMP,
				jobId: "job-1",
			}),
		);
		expect(stderrSpy).not.toHaveBeenCalled();
	});

	it("writes warn logs as JSON to stdout", () => {
		const stdoutSpy = vi.spyOn(console, "log").mockImplementation(() => {
			return;
		});
		const stderrSpy = vi.spyOn(console, "error").mockImplementation(() => {
			return;
		});
		const logger = createLogger();

		logger.warn("investigation retried", { retryCount: 2 });

		expect(stdoutSpy).toHaveBeenCalledWith(
			JSON.stringify({
				level: "WARN",
				message: "investigation retried",
				timestamp: FIXED_TIMESTAMP,
				retryCount: 2,
			}),
		);
		expect(stderrSpy).not.toHaveBeenCalled();
	});

	it("writes error logs as JSON to stderr", () => {
		const stdoutSpy = vi.spyOn(console, "log").mockImplementation(() => {
			return;
		});
		const stderrSpy = vi.spyOn(console, "error").mockImplementation(() => {
			return;
		});
		const logger = createLogger();

		logger.error("investigation failed", { jobId: "job-1" });

		expect(stderrSpy).toHaveBeenCalledWith(
			JSON.stringify({
				level: "ERROR",
				message: "investigation failed",
				timestamp: FIXED_TIMESTAMP,
				jobId: "job-1",
			}),
		);
		expect(stdoutSpy).not.toHaveBeenCalled();
	});
});
