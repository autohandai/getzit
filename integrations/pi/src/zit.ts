/**
 * Zit plumbing shared by the Pi extension and its tests.
 *
 * Everything here shells out to the `zit` binary (`$ZIT_BIN`, else `zit` on PATH)
 * and depends only on Node built-ins, so it can be tested without Pi.
 */

import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";

/** Same limits as Pi's built-in tools: 50KB or 2000 lines. */
const MAX_OUTPUT_BYTES = 50 * 1024;
const MAX_OUTPUT_LINES = 2000;

/** `zit run` keeps at most this much of an agent's account; do the same. */
export const MAX_SUMMARY_CHARS = 8000;

export const INSTALL_HINT =
	"Install it with `cargo install zit` (or build it from source with `cargo build --release`), " +
	"then put it on PATH or set ZIT_BIN to the binary.";

export interface ZitResult {
	stdout: string;
	stderr: string;
	code: number;
}

export interface RunOptions {
	cwd: string;
	signal?: AbortSignal;
	env?: NodeJS.ProcessEnv;
}

export class ZitMissingError extends Error {
	bin: string;

	constructor(bin: string) {
		super(`zit was not found (tried \`${bin}\`). ${INSTALL_HINT}`);
		this.name = "ZitMissingError";
		this.bin = bin;
	}
}

export function zitBin(env: NodeJS.ProcessEnv = process.env): string {
	return env.ZIT_BIN?.trim() || "zit";
}

/** Run zit. Resolves with any exit code; rejects only when zit cannot be started or is aborted. */
export function runZit(args: string[], options: RunOptions): Promise<ZitResult> {
	const env = options.env ?? process.env;
	const bin = zitBin(env);
	return new Promise((resolve, reject) => {
		execFile(
			bin,
			args,
			{ cwd: options.cwd, env, signal: options.signal, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
			(error, stdout, stderr) => {
				if (error) {
					const errno = error as NodeJS.ErrnoException;
					if (errno.code === "ENOENT" || errno.code === "EACCES") return reject(new ZitMissingError(bin));
					if (errno.name === "AbortError") return reject(error);
					if (typeof error.code !== "number") return reject(error);
				}
				resolve({ stdout, stderr, code: typeof error?.code === "number" ? error.code : 0 });
			},
		);
	});
}

/** Run zit and throw with its stderr when it exits non-zero. */
export async function runZitOk(args: string[], options: RunOptions): Promise<ZitResult> {
	const result = await runZit(args, options);
	if (result.code !== 0) {
		const message = (result.stderr || result.stdout).trim() || `exit code ${result.code}`;
		throw new Error(`zit ${args[0]} failed: ${message}`);
	}
	return result;
}

export function truncate(text: string): string {
	const lines = text.split("\n");
	let out = lines.length > MAX_OUTPUT_LINES ? lines.slice(0, MAX_OUTPUT_LINES).join("\n") : text;
	if (Buffer.byteLength(out) > MAX_OUTPUT_BYTES) out = Buffer.from(out).subarray(0, MAX_OUTPUT_BYTES).toString();
	if (out.length === text.length) return text;
	return `${out}\n\n[Output truncated to ${MAX_OUTPUT_LINES} lines / 50KB. Run \`zit status\` for the full list.]`;
}

// ---------------------------------------------------------------------------
// Workspaces
// ---------------------------------------------------------------------------

export interface ZitWorkspace {
	/** Workspace id, e.g. `546aa98a`. */
	id: string;
	/** The workspace's copy of the repository (`.../ws/<id>/tree`). */
	tree: string;
	intent?: string;
	agent?: string;
}

function realpath(path: string): string {
	try {
		return realpathSync(path);
	} catch {
		return path;
	}
}

function readMeta(wsDir: string): { id?: string; intent?: string; agent?: string } {
	try {
		return JSON.parse(readFileSync(join(wsDir, "meta.json"), "utf8"));
	} catch {
		return {};
	}
}

/**
 * The Zit workspace containing `cwd`, if any.
 *
 * Recognised by path shape (`.../ws/<id>/tree` with a sibling `meta.json`), from `cwd`
 * or any of its parents. Falls back to `$ZIT_WORKSPACE`, which `zit run` sets.
 */
export function findWorkspace(cwd: string, env: NodeJS.ProcessEnv = process.env): ZitWorkspace | undefined {
	let dir = realpath(cwd);
	for (;;) {
		if (basename(dir) === "tree") {
			const wsDir = dirname(dir);
			if (basename(dirname(wsDir)) === "ws" && existsSync(join(wsDir, "meta.json"))) {
				const meta = readMeta(wsDir);
				return { id: meta.id || basename(wsDir), tree: dir, intent: meta.intent, agent: meta.agent };
			}
		}
		const parent = dirname(dir);
		if (parent === dir) break;
		dir = parent;
	}
	const id = env.ZIT_WORKSPACE?.trim();
	return id ? { id, tree: realpath(cwd) } : undefined;
}

export function workspaceInstructions(ws: ZitWorkspace, autoRecord: boolean): string {
	const intent = ws.intent ? ` Its intent: "${ws.intent}".` : "";
	const finish = autoRecord
		? "When the task is complete, call zit_record with a short summary of what you did and why, and end your reply with the same summary. If you do not, your last reply is recorded as the summary when the session ends."
		: "When the task is complete, call zit_record with a short summary of what you did and why, and end your reply with the same summary. Nothing is recorded unless you call zit_record.";
	return [
		"## Zit workspace",
		"",
		`You are working in Zit workspace ${ws.id}, a disposable copy of the repository at ${ws.tree}.${intent}`,
		"You are one of several developers and agents changing this repository at the same time, coordinated through Zit.",
		"- Before you edit any file, claim it with zit_claim. To claim part of a file use `path#Symbol` for code or `path#Section heading` for Markdown.",
		"- If a claim is refused, someone else is already doing that part. Do not duplicate it. Pick a different part of the task that nobody holds, or stop if nothing useful is left.",
		"- zit_status shows what the others have claimed and are writing.",
		"- Do not commit, branch, stash or push with git. Zit records this workspace as a change.",
		`- ${finish}`,
	].join("\n");
}

// ---------------------------------------------------------------------------
// Commands behind the tools
// ---------------------------------------------------------------------------

export interface ClaimOutcome {
	granted: boolean;
	held: Array<{ resource: string; by?: { kind?: string; id?: string; agent?: string } }>;
	text: string;
}

export async function claim(ws: ZitWorkspace, resources: string[], signal?: AbortSignal, env?: NodeJS.ProcessEnv): Promise<ClaimOutcome> {
	if (resources.length === 0) throw new Error("zit_claim needs at least one resource");
	const args = ["claim", "--json", "--workspace", ws.id, ...resources];
	const result = await runZit(args, { cwd: ws.tree, signal, env });
	let parsed: { claim?: string; held?: ClaimOutcome["held"] } | undefined;
	try {
		parsed = JSON.parse(result.stdout);
	} catch {
		parsed = undefined;
	}
	if (!parsed?.claim) {
		const message = (result.stderr || result.stdout).trim() || `exit code ${result.code}`;
		throw new Error(`zit claim failed: ${message}`);
	}
	if (parsed.claim === "granted") {
		return { granted: true, held: [], text: `claimed: ${resources.join(", ")}` };
	}
	const held = parsed.held ?? [];
	const lines = held.map((h) => {
		const who = h.by ? `${h.by.agent ?? "someone"} (${h.by.kind ?? "workspace"} ${h.by.id ?? "?"})` : "someone else";
		return `  ${h.resource} is held by ${who}`;
	});
	return {
		granted: false,
		held,
		text: [
			"refused: nothing was claimed",
			...lines,
			"Do not edit these. Claim a different part of the task that nobody holds, or stop if nothing useful is left.",
		].join("\n"),
	};
}

export async function status(ws: ZitWorkspace | undefined, cwd: string, signal?: AbortSignal, env?: NodeJS.ProcessEnv): Promise<string> {
	const result = await runZitOk(["status"], { cwd: ws?.tree ?? cwd, signal, env });
	const header = ws ? `You are workspace ${ws.id}.\n\n` : "";
	return truncate(header + result.stdout.trimEnd());
}

export interface RecordOutcome {
	/** Id of the recorded change, or undefined when the workspace had nothing new. */
	change?: string;
	writes: string[];
	text: string;
}

export async function record(
	ws: ZitWorkspace,
	summary: string,
	options: { dispose?: boolean; signal?: AbortSignal; env?: NodeJS.ProcessEnv } = {},
): Promise<RecordOutcome> {
	const args = ["record", "--json", "--workspace", ws.id];
	const trimmed = summary.trim().slice(0, MAX_SUMMARY_CHARS);
	if (trimmed) args.push("--summary", trimmed);
	if (options.dispose) args.push("--dispose");
	const result = await runZitOk(args, { cwd: ws.tree, signal: options.signal, env: options.env });
	const parsed = JSON.parse(result.stdout) as { change: { id: string } | null; writes?: string[] };
	const writes = parsed.writes ?? [];
	if (!parsed.change) return { writes, text: "nothing to record: the workspace has no changes since the last record" };
	const lines = [`recorded change ${parsed.change.id}`];
	if (writes.length) lines.push(`wrote ${writes.join(", ")}`);
	return { change: parsed.change.id, writes, text: lines.join("\n") };
}

export async function isZitInitialised(repoCwd: string, signal?: AbortSignal): Promise<boolean> {
	return new Promise((resolve) => {
		execFile("git", ["rev-parse", "--verify", "--quiet", "refs/zit/current"], { cwd: repoCwd, signal }, (error) =>
			resolve(!error),
		);
	});
}

export async function gitTopLevel(cwd: string): Promise<string | undefined> {
	return new Promise((resolve) => {
		execFile("git", ["rev-parse", "--show-toplevel"], { cwd, encoding: "utf8" }, (error, stdout) =>
			resolve(error ? undefined : stdout.trim()),
		);
	});
}

export async function materialise(
	repoCwd: string,
	intent: string,
	options: { session?: string; signal?: AbortSignal; env?: NodeJS.ProcessEnv } = {},
): Promise<ZitWorkspace> {
	const args = ["materialise", "--json", "--agent", "pi", "--intent", intent];
	if (options.session) args.push("--session", options.session);
	const result = await runZitOk(args, { cwd: repoCwd, signal: options.signal, env: options.env });
	const meta = JSON.parse(result.stdout) as { id: string; path: string; intent?: string; agent?: string };
	return { id: meta.id, tree: meta.path, intent: meta.intent, agent: meta.agent };
}

// ---------------------------------------------------------------------------
// Pi session helpers
// ---------------------------------------------------------------------------

interface SessionEntryLike {
	type: string;
	message?: { role?: string; content?: unknown };
}

/** Text of the last assistant message on a session branch. */
export function lastAssistantText(entries: readonly SessionEntryLike[]): string {
	for (let i = entries.length - 1; i >= 0; i--) {
		const entry = entries[i];
		if (entry.type !== "message" || entry.message?.role !== "assistant") continue;
		const content = entry.message.content;
		if (typeof content === "string") return content.trim();
		if (!Array.isArray(content)) return "";
		return content
			.filter((c): c is { type: "text"; text: string } => c?.type === "text" && typeof c.text === "string")
			.map((c) => c.text)
			.join("\n")
			.trim();
	}
	return "";
}

/** Custom session entry type holding where a workspace session came from. */
export const ORIGIN_ENTRY = "zit-origin";

export interface ZitOrigin {
	/** The repository the workspace was materialised from. */
	repo: string;
	workspace?: string;
}

/**
 * Write a new Pi session file whose header cwd is `cwd`, so that `ctx.switchSession()`
 * restarts the session there. Format: docs/session-format.md (version 3).
 */
export function writeSessionFile(
	sessionDir: string,
	cwd: string,
	options: { parentSession?: string; origin?: ZitOrigin } = {},
): string {
	mkdirSync(sessionDir, { recursive: true });
	const id = randomUUID();
	const timestamp = new Date().toISOString();
	const header: Record<string, unknown> = { type: "session", version: 3, id, timestamp, cwd };
	if (options.parentSession) header.parentSession = options.parentSession;
	const lines = [JSON.stringify(header)];
	if (options.origin) {
		const entryId = randomUUID().replace(/-/g, "").slice(0, 8);
		lines.push(
			JSON.stringify({ type: "custom", id: entryId, parentId: null, timestamp, customType: ORIGIN_ENTRY, data: options.origin }),
		);
	}
	const file = join(sessionDir, `${timestamp.replace(/[:.]/g, "-")}_${id}.jsonl`);
	writeFileSync(file, `${lines.join("\n")}\n`, { flag: "wx" });
	return file;
}

export function findOrigin(entries: readonly { type: string; customType?: string; data?: unknown }[]): ZitOrigin | undefined {
	for (const entry of entries) {
		if (entry.type === "custom" && entry.customType === ORIGIN_ENTRY) {
			const data = entry.data as ZitOrigin | undefined;
			if (data?.repo) return data;
		}
	}
	return undefined;
}
