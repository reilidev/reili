/** @type {import("dependency-cruiser").IConfiguration} */
module.exports = {
	forbidden: [
		{
			name: "no-application-to-adapters",
			comment:
				"application layer must not depend on concrete adapter implementations",
			severity: "error",
			from: {
				path: "^src/application",
			},
			to: {
				path: "^src/adapters",
			},
		},
		{
			name: "no-ports-to-adapters",
			comment: "ports are contracts and must not depend on adapter implementations",
			severity: "error",
			from: {
				path: "^src/ports",
			},
			to: {
				path: "^src/adapters",
			},
		},
		{
			name: "no-ports-to-application",
			comment: "ports must not depend on application orchestration layer",
			severity: "error",
			from: {
				path: "^src/ports",
			},
			to: {
				path: "^src/application",
			},
		},
	],
	options: {
		doNotFollow: {
			path: "node_modules",
		},
		includeOnly: "^src",
		tsConfig: {
			fileName: "tsconfig.json",
		},
		tsPreCompilationDeps: true,
	},
};
