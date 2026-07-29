// TS mirrors of the server's JSON views.

export type VerdictView =
	| { kind: 'idea' }
	| { kind: 'ready' }
	| { kind: 'lead'; step: string; ready_date: string; ready_time: string }
	| { kind: 'shop'; tier: string | null; tier_name: string; items: string[] }
	| { kind: 'missing-equipment'; items: string[] };

export interface DishView {
	title: string;
	recipe: string | null;
	effort: string | null;
	unlinked: number;
	verdict: VerdictView;
}

export interface QueueEntryView {
	id: string;
	dishes: DishView[];
	reason: string | null;
	added: string;
	age_days: number | null;
}

export interface CoverageView {
	dinners: number;
	runs_out: string;
	freezer_dinners: number;
	runs_out_with_freezer: string;
}

export interface QueueView {
	location: string;
	headcount: number;
	entries: QueueEntryView[];
	coverage: CoverageView;
	someday: { id: string; titles: string[] }[];
}

export interface PageInfo {
	path: string;
	doc?: string;
	title?: string;
	tags?: Record<string, string>;
	effort?: string;
	retired?: boolean;
}

export interface ChangeInfo {
	hash: string;
	message: string;
	time: string | null;
}

export interface ThreadMessage {
	role: 'user' | 'assistant';
	content: string;
	created: string;
}
