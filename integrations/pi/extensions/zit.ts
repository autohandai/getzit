/**
 * Zit for Pi
 *
 * Lets Pi work alongside other developers and agents on one git repository through Zit:
 * - `/zit [intent]` (or `pi --zit "intent"`) materialises a Zit workspace and moves the session into it
 * - inside a workspace (however it was entered) the zit_claim, zit_status and zit_record tools
 *   are enabled and the system prompt explains how to share the repository
 * - when Pi quits inside a workspace, the work is recorded as a change with Pi's last reply
 *   as its summary, and the workspace is deleted (`/zit-auto-record off` or `--zit-no-record` to disable)
 *
 * Needs the `zit` binary on PATH, or `ZIT_BIN` pointing at it.
 */

import { writeFileSync } from "node:fs";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import {
	claim,
	findOrigin,
	findWorkspace,
	gitTopLevel,
	isZitInitialised,
	lastAssistantText,
	MAX_SUMMARY_CHARS,
	materialise,
	record,
	runZit,
	runZitOk,
	status,
	workspaceInstructions,
	writeSessionFile,
	type ZitWorkspace,
} from "../src/zit.ts";

const TOOL_NAMES = ["zit_claim", "zit_status", "zit_record"];
const AUTO_RECORD_ENTRY = "zit-auto-record";

function noWorkspaceError(): Error {
	return new Error(
		"This Pi session is not inside a Zit workspace. Start one with `/zit <intent>`, " +
			'or run `cd "$(zit materialise --agent pi --intent "...")" && pi`.',
	);
}

export default function zitExtension(pi: ExtensionAPI) {
	let workspace: ZitWorkspace | undefined;
	let autoRecord = true;

	pi.registerFlag("zit", {
		description: "Start the session in a new Zit workspace with this intent (same as /zit <intent>)",
		type: "string",
	});
	pi.registerFlag("zit-no-record", {
		description: "Do not record the Zit workspace as a change when Pi quits",
		type: "boolean",
		default: false,
	});

	function requireWorkspace(ctx: ExtensionContext): ZitWorkspace {
		const ws = workspace ?? findWorkspace(ctx.cwd);
		if (!ws) throw noWorkspaceError();
		return ws;
	}

	function autoRecordEnabled(ctx: ExtensionContext): boolean {
		if (pi.getFlag("zit-no-record") === true || process.env.PI_ZIT_NO_RECORD === "1") return false;
		let enabled = true;
		for (const entry of ctx.sessionManager.getEntries()) {
			if (entry.type === "custom" && entry.customType === AUTO_RECORD_ENTRY) {
				enabled = (entry.data as { enabled?: boolean } | undefined)?.enabled !== false;
			}
		}
		return enabled;
	}

	function syncTools() {
		const active = pi.getActiveTools().filter((name) => !TOOL_NAMES.includes(name));
		pi.setActiveTools(workspace ? [...active, ...TOOL_NAMES] : active);
	}

	// -------------------------------------------------------------------------
	// Tools
	// -------------------------------------------------------------------------

	pi.registerTool({
		name: "zit_claim",
		label: "Zit Claim",
		description:
			"Claim resources in the Zit workspace before editing them, so other agents working on the same repository do not duplicate the work. " +
			"Resources: `path`, `path#Symbol` (part of a code file) or `path#Section heading` (part of a Markdown file). " +
			"All or nothing: a refusal claims nothing and names who holds what.",
		promptSnippet: "Claim files or parts of files in the Zit workspace before editing them",
		promptGuidelines: [
			"Use zit_claim before editing any file in a Zit workspace; if zit_claim is refused, do not edit those resources.",
		],
		parameters: Type.Object({
			resources: Type.Array(Type.String({ description: "`path`, `path#Symbol` or `path#Section heading`" }), {
				description: "Resources to claim",
				minItems: 1,
			}),
		}),
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			const ws = requireWorkspace(ctx);
			const resources = params.resources.map((r) => r.replace(/^@/, ""));
			const outcome = await claim(ws, resources, signal);
			return {
				content: [{ type: "text", text: outcome.text }],
				details: { workspace: ws.id, resources, granted: outcome.granted, held: outcome.held },
			};
		},
	});

	pi.registerTool({
		name: "zit_status",
		label: "Zit Status",
		description:
			"Show the Zit graph: the current accepted state, recorded changes not yet accepted, and every open workspace with what it has claimed and is writing. " +
			"Output is truncated to 2000 lines or 50KB.",
		promptSnippet: "See what other agents in this repository have claimed, are writing, or have recorded",
		parameters: Type.Object({}),
		async execute(_toolCallId, _params, signal, _onUpdate, ctx) {
			const ws = requireWorkspace(ctx);
			const text = await status(ws, ctx.cwd, signal);
			return { content: [{ type: "text", text }], details: { workspace: ws.id } };
		},
	});

	pi.registerTool({
		name: "zit_record",
		label: "Zit Record",
		description:
			"Record the Zit workspace's current files as a change, with a summary of what was done and why. " +
			"The workspace stays open: later edits can be recorded again as a further change.",
		promptSnippet: "Record the work in this Zit workspace as a change, with a summary of what was done and why",
		promptGuidelines: ["Use zit_record once the task is complete, with a short summary of what you did and why."],
		parameters: Type.Object({
			summary: Type.String({ description: "What was done and why, in a few sentences" }),
		}),
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			const ws = requireWorkspace(ctx);
			const outcome = await record(ws, params.summary, { signal });
			return {
				content: [{ type: "text", text: outcome.text }],
				details: { workspace: ws.id, change: outcome.change, writes: outcome.writes },
			};
		},
	});

	// -------------------------------------------------------------------------
	// Session lifecycle
	// -------------------------------------------------------------------------

	pi.on("session_start", async (event, ctx) => {
		workspace = findWorkspace(ctx.cwd);
		autoRecord = autoRecordEnabled(ctx);
		syncTools();
		if (workspace) {
			ctx.ui.setStatus("zit", `zit ${workspace.id}${autoRecord ? "" : " (no auto-record)"}`);
			return;
		}
		ctx.ui.setStatus("zit", undefined);
		const intent = pi.getFlag("zit");
		if (event.reason === "startup" && typeof intent === "string") {
			if (!ctx.hasUI) {
				// Print and JSON modes run the prompt straight away; the session cannot move first.
				console.error(
					'pi-zit: --zit needs interactive or RPC mode. Use `zit run --agent pi --intent "..."`, ' +
						'or `cd "$(zit materialise --agent pi --intent "...")" && pi -p ...`.',
				);
				return;
			}
			// Session switching is only available to commands, so hand the flag to /zit.
			pi.sendUserMessage(`/zit ${intent}`.trimEnd(), { expandPromptTemplates: true });
		}
	});

	pi.on("before_agent_start", async (event) => {
		if (!workspace) return;
		return { systemPrompt: `${event.systemPrompt}\n\n${workspaceInstructions(workspace, autoRecord)}` };
	});

	pi.on("session_shutdown", async (event, ctx) => {
		const ws = workspace;
		workspace = undefined;
		if (!ws || event.reason !== "quit") return;
		const summary = lastAssistantText(ctx.sessionManager.getBranch()).slice(0, MAX_SUMMARY_CHARS);

		// Started by `zit run`: it records the workspace itself and reads the account from here.
		const summaryFile = process.env.ZIT_SUMMARY_FILE;
		if (summaryFile && process.env.ZIT_WORKSPACE === ws.id) {
			if (summary) writeFileSync(summaryFile, summary);
			return;
		}
		if (!autoRecord) {
			if (ctx.hasUI) ctx.ui.notify(`Zit workspace ${ws.id} left in place (auto-record is off).`, "info");
			return;
		}
		try {
			const outcome = await record(ws, summary, { dispose: true });
			if (ctx.hasUI) ctx.ui.notify(`Zit: ${outcome.text.split("\n")[0]}; workspace ${ws.id} deleted.`, "info");
		} catch (error) {
			if (ctx.hasUI) ctx.ui.notify(`Zit: could not record workspace ${ws.id}: ${(error as Error).message}`, "error");
		}
	});

	// -------------------------------------------------------------------------
	// Commands
	// -------------------------------------------------------------------------

	pi.registerCommand("zit", {
		description: "Start a Zit workspace for this session: /zit <intent>",
		handler: async (args, ctx) => {
			const here = findWorkspace(ctx.cwd);
			if (here) {
				ctx.ui.notify(`Already in Zit workspace ${here.id}. /zit-status shows the graph.`, "info");
				return;
			}
			const repo = await gitTopLevel(ctx.cwd);
			if (!repo) {
				ctx.ui.notify(`Zit needs a git repository; ${ctx.cwd} is not in one.`, "error");
				return;
			}
			if (!ctx.sessionManager.getSessionFile()) {
				ctx.ui.notify(
					'/zit needs a saved session (it was started with --no-session). Run: cd "$(zit materialise --agent pi --intent "...")" && pi',
					"error",
				);
				return;
			}
			let ws: ZitWorkspace | undefined;
			let switched = false;
			try {
				if (!(await isZitInitialised(repo))) {
					if (ctx.hasUI && !(await ctx.ui.confirm("Start Zit here?", `Runs \`zit init\` in ${repo}.`))) return;
					await runZitOk(["init"], { cwd: repo });
				}
				let intent = args.trim();
				if (!intent && ctx.hasUI) intent = ((await ctx.ui.input("What is this session for?", "intent")) ?? "").trim();

				await ctx.waitForIdle();
				const created = await materialise(repo, intent, { session: ctx.sessionManager.getSessionId() });
				ws = created;
				const file = writeSessionFile(ctx.sessionManager.getSessionDir(), created.tree, {
					parentSession: ctx.sessionManager.getSessionFile(),
					origin: { repo, workspace: created.id },
				});
				const result = await ctx.switchSession(file, {
					withSession: async (next) => {
						switched = true;
						next.ui.notify(`Zit workspace ${created.id}: ${created.tree}`, "info");
					},
				});
				if (result.cancelled) {
					await runZit(["dispose", created.id], { cwd: repo });
					ctx.ui.notify("Session switch cancelled; workspace disposed.", "warning");
				}
			} catch (error) {
				// Do not leave a workspace behind that no session is using.
				if (ws && !switched) await runZit(["dispose", ws.id], { cwd: repo }).catch(() => undefined);
				const message = `Zit: ${(error as Error).message}`;
				try {
					ctx.ui.notify(message, "error");
				} catch {
					// ctx is stale once the session has been replaced.
					console.error(message);
				}
			}
		},
	});

	pi.registerCommand("zit-status", {
		description: "Show zit status: changes, workspaces and their claims",
		handler: async (_args, ctx) => {
			try {
				ctx.ui.notify(await status(findWorkspace(ctx.cwd), ctx.cwd), "info");
			} catch (error) {
				ctx.ui.notify(`Zit: ${(error as Error).message}`, "error");
			}
		},
	});

	pi.registerCommand("zit-record", {
		description: "Record this Zit workspace as a change, delete it and go back to the repository: /zit-record [summary]",
		handler: async (args, ctx) => {
			const ws = findWorkspace(ctx.cwd);
			if (!ws) {
				ctx.ui.notify(noWorkspaceError().message, "error");
				return;
			}
			await ctx.waitForIdle();
			const summary = args.trim() || lastAssistantText(ctx.sessionManager.getBranch());
			const origin = findOrigin(ctx.sessionManager.getEntries());
			try {
				const outcome = await record(ws, summary, { dispose: true });
				ctx.ui.notify(`Zit: ${outcome.text}`, "info");
			} catch (error) {
				ctx.ui.notify(`Zit: ${(error as Error).message}`, "error");
				return;
			}
			if (!origin || !ctx.sessionManager.getSessionFile()) {
				ctx.ui.notify("The workspace directory is gone; quit Pi and start it again in the repository.", "warning");
				return;
			}
			// Nothing left to record when this session ends.
			workspace = undefined;
			const file = writeSessionFile(ctx.sessionManager.getSessionDir(), origin.repo, {
				parentSession: ctx.sessionManager.getSessionFile(),
			});
			await ctx.switchSession(file, {
				withSession: async (next) => next.ui.notify(`Back in ${origin.repo}`, "info"),
			});
		},
	});

	pi.registerCommand("zit-auto-record", {
		description: "Record the workspace when Pi quits: /zit-auto-record on|off",
		getArgumentCompletions: (prefix: string) =>
			["on", "off"].filter((v) => v.startsWith(prefix)).map((v) => ({ value: v, label: v })),
		handler: async (args, ctx) => {
			const value = args.trim().toLowerCase();
			if (value !== "on" && value !== "off") {
				ctx.ui.notify(`Auto-record is ${autoRecord ? "on" : "off"}. Use /zit-auto-record on|off.`, "info");
				return;
			}
			pi.appendEntry(AUTO_RECORD_ENTRY, { enabled: value === "on" });
			autoRecord = autoRecordEnabled(ctx);
			if (workspace) ctx.ui.setStatus("zit", `zit ${workspace.id}${autoRecord ? "" : " (no auto-record)"}`);
			ctx.ui.notify(
				autoRecord === (value === "on") ? `Zit auto-record ${value}.` : "Zit auto-record stays off (--zit-no-record or PI_ZIT_NO_RECORD=1).",
				"info",
			);
		},
	});
}
