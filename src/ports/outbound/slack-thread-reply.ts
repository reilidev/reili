export interface SlackThreadReplyInput {
	channel: string;
	threadTs: string;
	text: string;
}

export interface SlackThreadReplyPort {
	postThreadReply(input: SlackThreadReplyInput): Promise<void>;
}
