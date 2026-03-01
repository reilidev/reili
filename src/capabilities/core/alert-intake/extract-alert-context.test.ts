import { describe, expect, it } from "vitest";
import { extractAlertContext } from "./extract-alert-context";

describe("extractAlertContext", () => {
	it("returns trigger message text and empty thread transcript", () => {
		expect(
			extractAlertContext({
				triggerMessageText: "monitor alert",
				threadMessages: [],
			}),
		).toEqual({
			rawText: "monitor alert",
			triggerMessageText: "monitor alert",
			threadTranscript: "",
		});
	});

	it("trims surrounding whitespace", () => {
		expect(
			extractAlertContext({
				triggerMessageText: "  datadog monitor  ",
				threadMessages: [],
			}),
		).toEqual({
			rawText: "datadog monitor",
			triggerMessageText: "datadog monitor",
			threadTranscript: "",
		});
	});

	it("formats thread messages as transcript", () => {
		expect(
			extractAlertContext({
				triggerMessageText: "alert",
				threadMessages: [
					{
						ts: "1710000000.000001",
						user: "U123",
						text: "First message",
					},
					{
						ts: "1710000000.000002",
						text: " follow-up from bot ",
					},
				],
			}),
		).toEqual({
			rawText: "alert",
			triggerMessageText: "alert",
			threadTranscript:
				"[ts: 1710000000.000001 | iso: 2024-03-09T16:00:00.000Z] U123: First message\n---\n[ts: 1710000000.000002 | iso: 2024-03-09T16:00:00.000Z] system: follow-up from bot",
		});
	});

	it("appends (You) when author matches bot user id", () => {
		expect(
			extractAlertContext({
				triggerMessageText: "<@U999> investigate this alert",
				botUserId: "U999",
				threadMessages: [
					{
						ts: "1710000000.000010",
						user: "U999",
						text: "I started investigation",
					},
				],
			}),
		).toEqual({
			rawText: "<@U999> investigate this alert",
			triggerMessageText: "<@U999> investigate this alert",
			threadTranscript:
				"[ts: 1710000000.000010 | iso: 2024-03-09T16:00:00.000Z] U999 (You): I started investigation",
		});
	});
});
