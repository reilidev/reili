export interface Logger {
	info(message: string, meta?: Record<string, unknown>): void;
	warn(message: string, meta?: Record<string, unknown>): void;
	error(message: string, meta?: Record<string, unknown>): void;
}

function write(
	level: "INFO" | "WARN" | "ERROR",
	message: string,
	meta?: Record<string, unknown>,
): void {
	const payload = {
		level,
		message,
		timestamp: new Date().toISOString(),
		...meta,
	};

	const output = JSON.stringify(payload);
	if (level === "ERROR") {
		console.error(output);
		return;
	}

	console.log(output);
}

export function createLogger(): Logger {
	return {
		info(message, meta) {
			write("INFO", message, meta);
		},
		warn(message, meta) {
			write("WARN", message, meta);
		},
		error(message, meta) {
			write("ERROR", message, meta);
		},
	};
}
