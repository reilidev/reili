import type { RequestHandler } from "express";
import { z } from "zod";

const authorizationHeaderSchema = z.string().optional();

export interface WorkerInternalAuthMiddlewareInput {
	workerInternalToken: string;
}

export function createWorkerInternalAuthMiddleware(
	input: WorkerInternalAuthMiddlewareInput,
): RequestHandler {
	return (request, response, next) => {
		const bearerToken = readBearerToken(request.header("authorization"));
		if (bearerToken !== input.workerInternalToken) {
			response.status(401).send("Unauthorized");
			return;
		}

		next();
	};
}

function readBearerToken(
	authorizationHeader: string | undefined,
): string | undefined {
	const parsedHeader = authorizationHeaderSchema.safeParse(authorizationHeader);
	if (!parsedHeader.success || parsedHeader.data === undefined) {
		return undefined;
	}

	const tokenMatch = parsedHeader.data.match(/^Bearer\s+(.+)$/);
	return tokenMatch?.[1];
}
