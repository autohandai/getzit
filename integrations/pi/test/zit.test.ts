/**
 * Integration tests: a temp git repository, a temp ZIT_HOME and the real zit binary.
 *
 *   ZIT_BIN=/path/to/zit npm test
 *
 * ZIT_BIN defaults to the binary built in this repository (target/release/zit).
 */

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, before, describe, test } from "node:test";
import { fileURLToPath } from "node:url";
import zitExtension from "../extensions/zit.ts";
import {
	findOrigin,
	findWorkspace,
	lastAssistantText,
	materialise,
	ORIGIN_ENTRY,
	runZit,
	workspaceInstructions,
	writeSessionFile,
	ZitMissingError,
	type ZitWorkspace,
} from "../src/zit.ts";

const here = dirname(fileURLToPath(import.meta.url));
const defaultBin = resolve(here, "../../../target/release/zit");
process.env.ZIT_BIN ||= defaultBin;
delete process.env.ZIT_WORKSPACE;
delete process.env.ZIT_SUMMARY_FILE;
delete process.env.PI_ZIT_NO_RECORD;

const root = realpathSync(mkdtempSync(join(tmpdir(), "pi-zit-test-")));
const repo = join(root, "repo");
process.env.ZIT_HOME = join(root, "zit-home");

function git(...args: string[]) {
	execFileSync("git", ["-c", "user.name=test", "-c", "user.email=test@example.com", ...args], { cwd: repo, stdio: "pipe" });
}

function zitJson(args: string[], cwd = repo): any {
	return JSON.parse(execFileSync(process.env.ZIT_BIN!, [...args, "--json"], { cwd, encoding: "utf8" }));
}

// ---------------------------------------------------------------------------
// A minimal stand-in for Pi's ExtensionAPI: records what the extension registers.
// ---------------------------------------------------------------------------

type Handler = (event: any, ctx: any) => any;

function loadExtension() {
	const tools = new Map<string, any>();
	const commands = new Map<string, any>();
	const handlers = new Map<string, Handler[]>();
	const flags = new Map<string, unknown>();
	const appended: Array<{ customType: string; data: unknown }> = [];
	let activeTools = ["read", "bash", "edit", "write"];
	const pi = {
		registerTool: (tool: any) => tools.set(tool.name, tool),
		registerCommand: (name: string, options: any) => commands.set(name, options),
		registerFlag: (name: string, options: any) => flags.set(name, options.default),
		getFlag: (name: string) => flags.get(name),
		on: (event: string, handler: Handler) => handlers.set(event, [...(handlers.get(event) ?? []), handler]),
		getActiveTools: () => [...activeTools],
		setActiveTools: (names: string[]) => {
			activeTools = [...names];
		},
		appendEntry: (customType: string, data: unknown) => appended.push({ customType, data }),
		sendUserMessage: () => {},
	};
	zitExtension(pi as any);
	const emit = async (event: string, payload: any, ctx: any) => {
		let result: any;
		for (const handler of handlers.get(event) ?? []) result = (await handler(payload, ctx)) ?? result;
		return result;
	};
	return { tools, commands, flags, emit, appended, activeTools: () => activeTools };
}

function makeCtx(cwd: string, entries: any[] = []) {
	const notes: string[] = [];
	return {
		notes,
		ctx: {
			cwd,
			hasUI: false,
			mode: "print",
			ui: {
				notify: (message: string) => notes.push(message),
				setStatus: () => {},
			},
			sessionManager: {
				getEntries: () => entries,
				getBranch: () => entries,
				getSessionFile: () => undefined,
				getSessionDir: () => "",
				getSessionId: () => "test-session",
			},
		},
	};
}

function text(result: any): string {
	return result.content.map((c: any) => c.text).join("\n");
}

function assistant(text: string) {
	return { type: "message", message: { role: "assistant", content: [{ type: "text", text }] } };
}

// ---------------------------------------------------------------------------

before(() => {
	assert.ok(existsSync(process.env.ZIT_BIN!), `zit binary not found at ${process.env.ZIT_BIN}; build it with cargo build --release`);
	mkdirSync(repo, { recursive: true });
	git("init", "-q");
	writeFileSync(join(repo, "lib.rs"), "pub fn price() -> u32 {\n    10\n}\n\npub fn tax() -> u32 {\n    2\n}\n");
	writeFileSync(join(repo, "README.md"), "# Demo\n\n## Usage\n\nRun it.\n");
	git("add", "-A");
	git("commit", "-qm", "init");
	execFileSync(process.env.ZIT_BIN!, ["init"], { cwd: repo, stdio: "pipe" });
});

after(() => {
	rmSync(root, { recursive: true, force: true });
});

describe("workspace detection", () => {
	test("recognises .../ws/<id>/tree with meta.json, from the tree or below it", async () => {
		const ws = await materialise(repo, "detect me");
		assert.equal(findWorkspace(ws.tree, {})?.id, ws.id);
		mkdirSync(join(ws.tree, "sub", "dir"), { recursive: true });
		const nested = findWorkspace(join(ws.tree, "sub", "dir"), {});
		assert.equal(nested?.id, ws.id);
		assert.equal(nested?.tree, realpathSync(ws.tree));
		assert.equal(nested?.intent, "detect me");
		assert.equal(nested?.agent, "pi");
		await runZit(["dispose", ws.id], { cwd: repo });
	});

	test("is not fooled by an ordinary directory, and falls back to ZIT_WORKSPACE", () => {
		assert.equal(findWorkspace(repo, {}), undefined);
		const fake = join(root, "ws", "abc", "tree");
		mkdirSync(fake, { recursive: true });
		assert.equal(findWorkspace(fake, {}), undefined, "no meta.json, not a workspace");
		assert.equal(findWorkspace(repo, { ZIT_WORKSPACE: "deadbeef" })?.id, "deadbeef");
	});
});

describe("tools", () => {
	let ws1: ZitWorkspace;
	let ws2: ZitWorkspace;
	const ext = loadExtension();

	before(async () => {
		ws1 = await materialise(repo, "change the price");
		ws2 = await materialise(repo, "also change the price");
	});

	after(async () => {
		await runZit(["dispose", "--all"], { cwd: repo });
	});

	test("registers zit_claim, zit_status and zit_record", () => {
		assert.deepEqual([...ext.tools.keys()].sort(), ["zit_claim", "zit_record", "zit_status"]);
		assert.deepEqual([...ext.commands.keys()].sort(), ["zit", "zit-auto-record", "zit-record", "zit-status"]);
	});

	test("zit_claim is granted to the first workspace and refused to the second", async () => {
		const claimTool = ext.tools.get("zit_claim");
		const first = await claimTool.execute("1", { resources: ["lib.rs#price", "README.md#Usage"] }, undefined, undefined, makeCtx(ws1.tree).ctx);
		assert.equal(first.details.granted, true);
		assert.match(text(first), /^claimed: lib\.rs#price, README\.md#Usage$/);

		const second = await claimTool.execute("2", { resources: ["lib.rs#price"] }, undefined, undefined, makeCtx(ws2.tree).ctx);
		assert.equal(second.details.granted, false);
		assert.match(text(second), /^refused: nothing was claimed/);
		assert.match(text(second), new RegExp(`lib\\.rs#price is held by pi \\(workspace ${ws1.id}\\)`));
		assert.equal(second.details.held[0].by.id, ws1.id);

		// A different symbol in the same file is still free.
		const other = await claimTool.execute("3", { resources: ["lib.rs#tax"] }, undefined, undefined, makeCtx(ws2.tree).ctx);
		assert.equal(other.details.granted, true);
	});

	test("zit_status lists both workspaces and their claims", async () => {
		const result = await ext.tools.get("zit_status").execute("4", {}, undefined, undefined, makeCtx(ws2.tree).ctx);
		const out = text(result);
		assert.match(out, new RegExp(`^You are workspace ${ws2.id}\\.`));
		assert.match(out, /WORKSPACES \(2\)/);
		assert.ok(out.includes(ws1.id) && out.includes(ws2.id));
		assert.match(out, /claims README\.md#Usage, lib\.rs#price/);
		assert.match(out, /claims lib\.rs#tax/);
	});

	test("zit_record records a change carrying the summary; the workspace stays open", async () => {
		writeFileSync(join(ws1.tree, "lib.rs"), "pub fn price() -> u32 {\n    12\n}\n\npub fn tax() -> u32 {\n    2\n}\n");
		const summary = "Raised the price to 12 because the supplier raised theirs.";
		const result = await ext.tools.get("zit_record").execute("5", { summary }, undefined, undefined, makeCtx(ws1.tree).ctx);
		assert.match(text(result), /^recorded change [0-9a-f]{40}\nwrote lib\.rs#price$/);
		const change = zitJson(["show", result.details.change]);
		assert.equal(change.summary, summary);
		assert.equal(change.intent, "change the price");
		assert.equal(change.agent, "pi");
		assert.deepEqual(change.writes, ["lib.rs#price"]);
		assert.ok(existsSync(ws1.tree), "zit_record does not dispose the workspace");

		const again = await ext.tools.get("zit_record").execute("6", { summary }, undefined, undefined, makeCtx(ws1.tree).ctx);
		assert.equal(again.details.change, undefined);
		assert.match(text(again), /^nothing to record/);
	});

	test("tools refuse to run outside a workspace", async () => {
		for (const name of ["zit_claim", "zit_status", "zit_record"]) {
			await assert.rejects(
				ext.tools.get(name).execute("7", { resources: ["lib.rs"], summary: "x" }, undefined, undefined, makeCtx(repo).ctx),
				/not inside a Zit workspace/,
			);
		}
	});

	test("a missing zit binary gives an install hint", async () => {
		await assert.rejects(
			runZit(["status"], { cwd: repo, env: { ...process.env, ZIT_BIN: join(root, "no-such-zit") } }),
			(error: unknown) => error instanceof ZitMissingError && /cargo install zit/.test(error.message) && /ZIT_BIN/.test(error.message),
		);
	});
});

describe("session lifecycle", () => {
	test("inside a workspace: tools enabled and instructions added to the system prompt", async () => {
		const ws = await materialise(repo, "prompt test");
		const ext = loadExtension();
		await ext.emit("session_start", { type: "session_start", reason: "startup" }, makeCtx(ws.tree).ctx);
		assert.deepEqual(ext.activeTools(), ["read", "bash", "edit", "write", "zit_claim", "zit_status", "zit_record"]);
		const result = await ext.emit("before_agent_start", { systemPrompt: "BASE" }, makeCtx(ws.tree).ctx);
		assert.ok(result.systemPrompt.startsWith("BASE\n\n## Zit workspace"));
		for (const phrase of [ws.id, "several developers and agents", "claim it with zit_claim", "If a claim is refused", "zit_status", "Do not commit", "summary of what you did and why"]) {
			assert.ok(result.systemPrompt.includes(phrase), `system prompt mentions ${phrase}`);
		}
		await runZit(["dispose", ws.id], { cwd: repo });
	});

	test("outside a workspace: tools disabled and the system prompt untouched", async () => {
		const ext = loadExtension();
		await ext.emit("session_start", { type: "session_start", reason: "startup" }, makeCtx(repo).ctx);
		assert.deepEqual(ext.activeTools(), ["read", "bash", "edit", "write"]);
		assert.equal(await ext.emit("before_agent_start", { systemPrompt: "BASE" }, makeCtx(repo).ctx), undefined);
	});

	test("quitting records the workspace with the last assistant message and deletes it", async () => {
		const ws = await materialise(repo, "quit test");
		writeFileSync(join(ws.tree, "NOTES.md"), "# Notes\n");
		const entries = [assistant("first reply"), { type: "message", message: { role: "user", content: "go" } }, assistant("Added NOTES.md so decisions are written down.")];
		const ext = loadExtension();
		const { ctx } = makeCtx(ws.tree, entries);
		await ext.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
		await ext.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx);
		assert.ok(!existsSync(ws.tree), "workspace deleted");
		const recorded = zitJson(["status"]).changes.find((c: any) => c.intent === "quit test");
		assert.ok(recorded, "change recorded");
		assert.equal(recorded.summary, "Added NOTES.md so decisions are written down.");
	});

	test("session switches other than quit do not record", async () => {
		const ws = await materialise(repo, "reload test");
		const ext = loadExtension();
		const { ctx } = makeCtx(ws.tree, [assistant("x")]);
		await ext.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
		await ext.emit("session_shutdown", { type: "session_shutdown", reason: "reload" }, ctx);
		assert.ok(existsSync(ws.tree));
		await runZit(["dispose", ws.id], { cwd: repo });
	});

	test("auto-record off leaves the workspace in place", async () => {
		const ws = await materialise(repo, "no auto record");
		writeFileSync(join(ws.tree, "KEEP.md"), "# Keep\n");
		const ext = loadExtension();
		const entries: any[] = [{ type: "custom", customType: "zit-auto-record", data: { enabled: false } }, assistant("done")];
		const { ctx } = makeCtx(ws.tree, entries);
		await ext.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
		const prompt = await ext.emit("before_agent_start", { systemPrompt: "" }, ctx);
		assert.match(prompt.systemPrompt, /Nothing is recorded unless you call zit_record/);
		await ext.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx);
		assert.ok(existsSync(ws.tree));
		assert.equal(zitJson(["status"]).changes.find((c: any) => c.intent === "no auto record"), undefined);
		await runZit(["dispose", ws.id], { cwd: repo });
	});

	test("started by `zit run`: writes the summary to ZIT_SUMMARY_FILE instead of recording", async () => {
		const ws = await materialise(repo, "zit run test");
		const summaryFile = join(root, "summary.txt");
		process.env.ZIT_WORKSPACE = ws.id;
		process.env.ZIT_SUMMARY_FILE = summaryFile;
		try {
			const ext = loadExtension();
			const { ctx } = makeCtx(ws.tree, [assistant("Did the thing because it was asked.")]);
			await ext.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
			await ext.emit("session_shutdown", { type: "session_shutdown", reason: "quit" }, ctx);
			assert.equal(readFileSync(summaryFile, "utf8"), "Did the thing because it was asked.");
			assert.ok(existsSync(ws.tree), "left for zit run to record");
		} finally {
			delete process.env.ZIT_WORKSPACE;
			delete process.env.ZIT_SUMMARY_FILE;
			await runZit(["dispose", ws.id], { cwd: repo });
		}
	});
});

describe("helpers", () => {
	test("lastAssistantText joins the text parts of the last assistant message", () => {
		const entries = [
			assistant("old"),
			{ type: "message", message: { role: "assistant", content: [{ type: "thinking", thinking: "hmm" }, { type: "text", text: "a" }, { type: "toolCall" }, { type: "text", text: "b" }] } },
			{ type: "message", message: { role: "toolResult", content: [{ type: "text", text: "tool" }] } },
		];
		assert.equal(lastAssistantText(entries), "a\nb");
		assert.equal(lastAssistantText([]), "");
	});

	test("writeSessionFile writes a v3 header with the cwd and an origin entry", () => {
		const dir = join(root, "sessions");
		const file = writeSessionFile(dir, "/some/ws/tree", { parentSession: "/old.jsonl", origin: { repo, workspace: "abc" } });
		const [header, entry] = readFileSync(file, "utf8").trim().split("\n").map((l) => JSON.parse(l));
		assert.equal(header.type, "session");
		assert.equal(header.version, 3);
		assert.equal(header.cwd, "/some/ws/tree");
		assert.equal(header.parentSession, "/old.jsonl");
		assert.equal(entry.type, "custom");
		assert.equal(entry.customType, ORIGIN_ENTRY);
		assert.match(entry.id, /^[0-9a-f]{8}$/);
		assert.equal(entry.parentId, null);
		assert.deepEqual(findOrigin([entry]), { repo, workspace: "abc" });
	});

	test("instructions mention auto-record only when it is on", () => {
		const ws = { id: "abc", tree: "/t", intent: "do it" };
		assert.match(workspaceInstructions(ws, true), /your last reply is recorded as the summary/);
		assert.match(workspaceInstructions(ws, false), /Nothing is recorded unless you call zit_record/);
		assert.match(workspaceInstructions(ws, true), /Its intent: "do it"/);
	});
});
