// The version readers can install: the latest GitHub release, which the release workflow also
// publishes to crates.io. Read once per build. Falls back to Cargo.toml, with a warning, when
// GitHub cannot be reached. GITHUB_TOKEN, when set, avoids GitHub's anonymous rate limit.
import { readFileSync } from "node:fs";
import { join } from "node:path";

const REPO = "autohandai/getzit";
let cached: Promise<string> | undefined;

export function zitVersion(): Promise<string> {
  cached ??= latestRelease();
  return cached;
}

async function latestRelease(): Promise<string> {
  const token = process.env.GITHUB_TOKEN;
  try {
    const response = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
      headers: { Accept: "application/vnd.github+json", ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    });
    if (response.ok) {
      const { tag_name: tag } = (await response.json()) as { tag_name?: string };
      if (tag) return tag.replace(/^v/, "");
    }
    console.warn(`zit-version: GitHub answered ${response.status}; using Cargo.toml`);
  } catch (error) {
    console.warn(`zit-version: GitHub unreachable (${error}); using Cargo.toml`);
  }
  const cargo = readFileSync(join(process.cwd(), "Cargo.toml"), "utf8");
  return /^version\s*=\s*"([^"]+)"/m.exec(cargo)?.[1] ?? "unknown";
}
