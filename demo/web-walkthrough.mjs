import puppeteer from "puppeteer-core";
const out = process.argv[2];
const browser = await puppeteer.launch({ executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: "new" });
const page = await browser.newPage();
await page.setViewport({ width: 1600, height: 1000, deviceScaleFactor: 1 });
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

await sleep(1500);
await caption("zit web: one repository, live. Accepted changes form the line; waiting changes sit above it, people at work below.");
await shot(4500);
if (!await pick("hana")) throw new Error("no hana row");
await caption("A waiting change: what it wrote (down to the section), its checks, and its author's own reason.");
await shot(4500);
if (!await pick("jun")) throw new Error("no jun row");
await caption("A rejected change says why (README.md does not merge with current), shows the diff, and gives the command to run next.");
await shot(5000);
if (!await pick("kai")) throw new Error("no kai row");
await caption("Someone still at work: what kai claimed and is writing right now, visible before anything is recorded.");
await shot(5000);
await page.keyboard.press("Escape");
await page.focus("#search"); await page.keyboard.type("Prices", { delay: 60 });
await caption("Filter by person, file or section: chen's Prices change landed, jun's collided with it.");
await shot(4500);
await page.click("#search", { clickCount: 3 }); await page.keyboard.press("Backspace");
await page.click("#theme");
await caption("Light or dark. Read-only, on localhost only: it watches, it never changes anything.");
await shot(4000);
await browser.close();
