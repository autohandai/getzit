# Contributing to Zit

Thank you for helping. Zit is small on purpose: every behaviour has a test, every claim in the docs has a measurement or a test behind it, and every design decision has an ADR.

## Set up

```sh
git clone https://github.com/autohandai/getzit && cd getzit
cargo build                      # Rust 1.88+, git 2.38+
cargo test                       # unit and integration tests, against real git repositories
npm install && npm run dev       # the manual, at http://localhost:4321
```

On Linux, the copy-on-write tests run only on a reflink file system (btrfs, XFS with reflink, bcachefs); elsewhere they report that they were skipped. CI runs them on btrfs.

## Make a change

1. **Open an issue first** for anything larger than a fix, so we can agree on the approach. For design changes, write or amend an ADR in [`adr/`](adr/).
2. **Test first.** Write the test that fails without your change, watch it fail, then make it pass. Integration tests live in [`tests/`](tests/) and drive real git repositories; see `tests/common/mod.rs` for the fixtures.
3. **Keep the bar:**
   ```sh
   cargo fmt --check
   cargo clippy --all-targets --features bench --locked -- -D warnings
   cargo test --locked
   ```
4. **Docs with code.** A change in behaviour updates the manual in [`docs/`](docs/) and, if it is a limit, [`docs/limits.mdx`](docs/limits.mdx). Claims need evidence: a test name or a reproducible measurement. Run `npx blume validate` for broken links.
5. **Changelog.** Add a line under `## Unreleased` in [CHANGELOG.md](CHANGELOG.md).

## Sign your commits off

Like git itself, Zit uses the [Developer Certificate of Origin](https://developercertificate.org/). Every commit must carry a `Signed-off-by` line, which certifies that you wrote the change or have the right to submit it under the project's license (GPL-2.0-only):

```sh
git commit -s -m "Explain what and why"
```

A check on every pull request refuses commits without it. To fix a branch: `git rebase --signoff main`.

## Pull requests

- One topic per pull request; explain what changed and why, and how you tested it.
- CI must pass on macOS, Linux and Linux with btrfs.
- A maintainer reviews within a few working days. Two kinds of change need extra care and an ADR: anything that changes what `accept` lets through, and anything that changes what is stored in `refs/zit/*`.

## Collaborate

- **Questions and ideas:** GitHub Discussions, or an issue with the `question` label.
- **Bugs:** the bug template asks for `zit --version`, `git --version`, the OS and file system, and the smallest steps to reproduce.
- **Agent integrations:** new presets for `zit run`, MCP clients and extensions are welcome; include a test with a fake agent binary (see `tests/usage.rs`).

## Releases

Maintainers release from `main`:

1. Move `## Unreleased` in CHANGELOG.md to `## x.y.z - date`, and bump `version` in Cargo.toml.
2. Commit (`Release x.y.z`), then tag and push: `git tag -s vx.y.z -m "Zit x.y.z" && git push origin main vx.y.z`.
3. The [release workflow](.github/workflows/release.yml) checks that the tag matches Cargo.toml, runs the tests, builds binaries for macOS (arm64, x86_64) and Linux (x86_64, arm64) with SHA-256 checksums, creates the GitHub release with the changelog section as notes, and publishes the crate to crates.io (using the `CARGO_REGISTRY_TOKEN` repository secret).

Versions follow [Semantic Versioning](https://semver.org/). Until 1.0, a minor version may change the CLI or the `refs/zit/*` layout; the changelog says how to migrate.
