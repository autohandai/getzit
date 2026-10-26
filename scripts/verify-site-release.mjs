// A build can succeed with an old GitHub Releases snapshot when the network
// is unavailable. Never deploy a version label without its release notes.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

const root = process.cwd();
const cargo = await readFile(join(root, "Cargo.toml"), "utf8");
const version = /^version\s*=\s*"([^"]+)"/m.exec(cargo)?.[1];
assert(version, "Cannot determine the release version from Cargo.toml");

const slug = `v${version}`.toLowerCase().replace(/[^a-z0-9]+/g, "-");
const route = `/changelog/${slug}`;
const timeline = await readFile(join(root, "dist/changelog/index.html"), "utf8");
assert(
  timeline.includes(`href="${route}"`) || timeline.includes(`href="${route}/"`),
  `Site build is stale: the changelog has no ${version} release link. Rebuild with GitHub access before deploying.`,
);
const release = await readFile(join(root, "dist", "changelog", slug, "index.html"), "utf8");
assert(release.includes(`Zit ${version}`), `The ${version} release page is missing its title`);
console.log(`Site release verified: Zit ${version} at ${route}`);
