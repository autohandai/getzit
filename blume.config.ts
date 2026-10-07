import { defineConfig } from "blume";
import { filesystem, githubReleases } from "blume/sources";
import { zitVersion } from "./components/zit-version";

// The latest GitHub release, the same version the docs pages show.
const version = await zitVersion();

export default defineConfig({
  title: "Zit",
  description: "Zit: a git extension for many developers and coding agents changing one repository at once.",
  content: {
    sources: [
      filesystem({ root: "docs" }),
      // Each GitHub release, whose notes come from CHANGELOG.md, is a page under /changelog.
      // The repository is private: the build reads GITHUB_TOKEN (see the deploy script).
      githubReleases({ prefix: "changelog", owner: "autohandai", repo: "getzit" }),
    ],
  },
  changelog: {
    title: "Changelog",
    description: "Every Zit release, from GitHub. The notes come from CHANGELOG.md in the repository.",
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
      { label: `Zit ${version}`, href: "/changelog" },
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
        items: [
          { label: "Changelog", href: "/changelog" },
          "license",
          "legal",
          { label: "Autohand AI, sponsor", href: "https://autohand.ai" },
        ],
      },
    ],
  },
});
