import type { App } from "@slack/bolt";
import { describe, expect, it, vi } from "vitest";
import { SlackThreadHistoryAdapter } from "./slack-thread-history-adapter";

type SlackWebClient = {
	conversations: Pick<App["client"]["conversations"], "replies">;
};
type RepliesArgs = Parameters<SlackWebClient["conversations"]["replies"]>[0];
type ConversationsRepliesResponse = Awaited<
	ReturnType<SlackWebClient["conversations"]["replies"]>
>;

function createClientMock() {
	const replies =
		vi.fn<(input: RepliesArgs) => Promise<ConversationsRepliesResponse>>();
	const client: SlackWebClient = {
		conversations: {
			replies,
		},
	};

	return {
		client,
		replies,
	};
}

describe("SlackThreadHistoryAdapter", () => {
	it("merges paginated replies in returned order", async () => {
		const { client, replies } = createClientMock();
		replies
			.mockResolvedValueOnce({
				ok: true,
				messages: [
					{
						ts: "1710000000.000001",
						user: "U1",
						text: "first",
					},
				],
				response_metadata: {
					next_cursor: "cursor-2",
				},
			})
			.mockResolvedValueOnce({
				ok: true,
				messages: [
					{
						ts: "1710000000.000002",
						user: "U2",
						text: "second",
					},
					{
						ts: "1710000000.000003",
						user: "U3",
						text: "third",
					},
				],
				response_metadata: {
					next_cursor: "",
				},
			});

		const adapter = new SlackThreadHistoryAdapter(client);
		const result = await adapter.fetchThreadHistory({
			channel: "C123",
			threadTs: "1710000000.000000",
		});

		expect(replies).toHaveBeenCalledTimes(2);
		expect(result).toEqual([
			{
				ts: "1710000000.000001",
				user: "U1",
				text: "first",
			},
			{
				ts: "1710000000.000002",
				user: "U2",
				text: "second",
			},
			{
				ts: "1710000000.000003",
				user: "U3",
				text: "third",
			},
		]);
	});
});
