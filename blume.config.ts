import { readFileSync } from "node:fs";
import { defineConfig } from "blume";

// The released version, from Cargo.toml, so the footer never drifts from the crate.
const version = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync(new URL("./Cargo.toml", import.meta.url), "utf8"))?.[1];

export default defineConfig({
  title: "Zit",
  description: "Zit: a git extension for many developers and coding agents changing one repository at once.",
  content: {
    root: "docs",
  },
  // Cloudflare Workers does not expose the site's URL; canonical links, the sitemap and llms.txt need it.
  deployment: {
    site: "https://getzit.org",
  },
  ai: {
    // Autohand Dev is added first by components/OpenInAutohand.astro; v0 goes last.
    openInChat: ["chatgpt", "claude", "t3", "cursor", "v0"],
  },
  footer: {
    links: [
      { label: `Zit ${version}`, href: "https://crates.io/crates/zit" },
      { label: "GPL-2.0-only", href: "/license" },
      { label: "Legal", href: "/legal" },
      { label: `© ${new Date().getFullYear()} Autohand AI`, href: "https://autohand.ai" },
    ],
  },
  navigation: {
    sidebar: [
      { label: "Get started", items: ["index", "tutorial", "quickstart", "why", "compare"] },
      { label: "Use it", items: ["agents", "web", "git"] },
      { label: "How it works", items: ["concepts", "science", "research", "prior-art"] },
      { label: "Evidence", items: ["benchmarks", "lessons", "limits"] },
      { label: "Reference", items: ["cli", "checks", "spec"] },
      { label: "Roadmap", items: ["roadmap"] },
      {
        label: "Project",
        items: ["license", "legal", { label: "Autohand AI, sponsor", href: "https://autohand.ai" }],
      },
    ],
  },
});
