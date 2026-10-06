import puppeteer from "puppeteer-core";
const out = process.argv[2];
const browser = await puppeteer.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: "new" });
const page = await browser.newPage();
await page.setViewport({ width: 1600, height: 1120, deviceScaleFactor: 1 });
await page.emulateMediaFeatures([{ name: "prefers-color-scheme", value: "dark" }]);
await page.goto("http://127.0.0.1:4799/", { waitUntil: "networkidle0" });
const sleep = ms => new Promise(r => setTimeout(r, ms));
let n = 0;
const caption = async (text) => page.evaluate(t => {
  let c = document.getElementById("demo-caption");
  if (!c) {
    c = document.createElement("div"); c.id = "demo-caption";
    Object.assign(c.style, { position: "fixed", left: "50%", bottom: "22px", transform: "translateX(-50%)", maxWidth: "980px",
      padding: "12px 18px", borderRadius: "10px", background: "rgba(10,10,10,.88)", color: "#fff", font: "500 17px/1.45 'Autohand Sans', system-ui",
      boxShadow: "0 6px 30px rgba(0,0,0,.35)", zIndex: 99999, textAlign: "center", border: "1px solid rgba(255,255,255,.12)" });
    document.body.appendChild(c);
  }
  c.textContent = t;
}, text);
const shot = async (holdMs) => { await sleep(350); await page.screenshot({ path: `${out}/s${String(++n).padStart(2, "0")}-${holdMs}.png` }); };
const pick = async (who) => page.evaluate(w => {
  const row = [...document.querySelectorAll("#rows tr")].find(r => r.textContent.includes(w));
  row?.click(); return !!row;
}, who);


import { writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
// The Autohand fonts in src/web/fonts (ZIT_FONTS when this script is run from elsewhere).
const fonts = pathToFileURL((process.env.ZIT_FONTS ?? new URL("../src/web/fonts", import.meta.url).pathname) + "/").href;
const cardFile = join(tmpdir(), `zit-card-${process.pid}.html`);
const card = async (html, holdMs) => {
  const doc = `<!doctype html><meta charset="utf-8"><style>
    @font-face { font-family: "Autohand Sans"; src: url("${fonts}autohand-sans.woff2") format("woff2"); }
    @font-face { font-family: "Autohand Mono"; src: url("${fonts}autohand-mono.woff2") format("woff2"); }
    html, body { margin: 0; height: 100%; background: #181818; color: #ededed; font-family: "Autohand Sans", system-ui; }
    main { box-sizing: border-box; height: 100%; padding: 96px 120px; display: flex; flex-direction: column; justify-content: center; }
    h1 { font-size: 64px; font-weight: 600; letter-spacing: -0.02em; margin: 0 0 20px; }
    h2 { font-size: 34px; font-weight: 500; margin: 0 0 44px; color: #ededed; }
    p.lead { font-size: 24px; color: #a3a3a3; margin: 0; max-width: 1100px; line-height: 1.5; }
    .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 28px 48px; }
    .f { border-top: 1px solid #333; padding-top: 18px; }
    .f b { display: block; font-size: 25px; font-weight: 600; margin-bottom: 8px; }
    .f span { font-size: 19px; color: #a3a3a3; line-height: 1.5; }
    code { font-family: "Autohand Mono", ui-monospace; color: #7ee787; }
    .dot { display: inline-block; width: 14px; height: 14px; border-radius: 50%; background: #7ee787; margin-left: 8px; vertical-align: super; }
  </style><main>${html}</main>`;
  writeFileSync(cardFile, doc);
  await page.goto(pathToFileURL(cardFile).href, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
  await shot(holdMs);
};

await card(`<h1>zit<span class="dot"></span></h1>
  <h2>Many sessions on one repository, at the same time. No worktrees.</h2>
  <p class="lead">People and coding agents each work in their own copy of the code, see what the others are changing,
  and land their work one change at a time, every change with its reason.</p>`, 4500);
await card(`<div class="grid">
  <div class="f"><b>A copy for every session</b><span>A copy-on-write clone of one checkout: a copy of a copy, sharing every file and installed dependency until it is edited. No worktree, no new files.</span></div>
  <div class="f"><b>Sessions see each other</b><span>What each session is writing, and the parts it has claimed, are visible to the others while they work, so they split the work instead of repeating it.</span></div>
  <div class="f"><b>Changes land one at a time</b><span>Each is checked against what landed meanwhile and against your tests. A collision is refused with the reason, and nothing is lost.</span></div>
  <div class="f"><b>Every change explains itself</b><span>Its author's own account of what it did and why, and what the agent's turn cost. All of it is plain git underneath.</span></div>
</div>`, 7000);
await card(`<h2>Ten sessions changed one <code>README.md</code> at the same time.</h2>
  <p class="lead">Three more are still working. This is <code>zit web</code>, watching that repository.</p>`, 3500);

await page.goto("http://127.0.0.1:4799/", { waitUntil: "networkidle0" });
await sleep(1500);
await caption("Five changes have landed on the line, four wait their turn above it, one collided (red), and three sessions are still working below. None of them is a git worktree.");
await shot(4500);
if (!await pick("hana")) throw new Error("no hana row");
await caption("A waiting change: what it wrote, down to the section, its author's reason, and the command that lands it.");
await shot(4500);
if (!await pick("jun")) throw new Error("no jun row");
await caption("jun and chen both rewrote the Prices section. chen landed first, so jun's change is refused with the reason, the diff, and the command that redoes it on top. Nothing is lost.");
await shot(5000);
if (!await pick("kai")) throw new Error("no kai row");
await caption("kai is still working, in a copy of their own. What kai has claimed and is writing right now is visible to every other session before anything is recorded.");
await shot(5000);
await page.keyboard.press("Escape");
await page.focus("#search"); await page.keyboard.type("Prices", { delay: 60 });
await caption("Filter by person, file or section: chen's Prices change landed, jun's collided with it.");
await shot(4500);
await page.click("#search", { clickCount: 3 }); await page.keyboard.press("Backspace");
await page.click("#theme");
await caption("Light or dark. zit web runs on your machine and only reads: it never changes the repository.");
await shot(4000);
await browser.close();
