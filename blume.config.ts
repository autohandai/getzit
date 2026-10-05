import { defineConfig } from "blume";

export default defineConfig({
  title: "Zit",
  description: "Zit: a git extension for many developers and coding agents changing one repository at once.",
  content: {
    root: "docs",
  },
  navigation: {
    sidebar: [
      { label: "Get started", items: ["index", "tutorial", "quickstart", "why"] },
      { label: "Use it", items: ["agents", "web", "git"] },
      { label: "How it works", items: ["concepts", "science", "research"] },
      { label: "Evidence", items: ["benchmarks", "lessons", "criticism", "limits"] },
      { label: "Reference", items: ["cli", "checks", "spec"] },
    ],
  },
});
