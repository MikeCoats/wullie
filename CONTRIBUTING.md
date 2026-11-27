<!--
SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Contributing to Wullie

## Prerequisites

- [Rust][rust] via `rustup`, which installs the version pinned in `rust-toolchain.toml`
- [`just`][just]: e.g. `cargo install just`, or `cargo binstall just` for a prebuilt binary
- [`reuse`][reuse] e.g. `pipx install reuse`
- [`shellcheck`][shellcheck] e.g. `sudo apt install shellcheck` or `brew install shellcheck`
- For `just deb` only, on Debian or Ubuntu: `dpkg-dev` and `lintian`, e.g. `sudo apt install dpkg-dev lintian`

## Common Recipes

| Command            | What it does                                        |
|--------------------|-----------------------------------------------------|
| `just`             | List the available recipes                          |
| `just ci`          | Run every check that CI runs                        |
| `just fmt`         | Check formatting with `cargo fmt --check`           |
| `just lint`        | Lint with clippy, treating warnings as errors       |
| `just shellcheck`  | Check the packaging scripts in `pkg/`               |
| `just test`        | Run the test suite                                  |
| `just reuse`       | Check REUSE licensing compliance with `reuse lint`  |
| `just package`     | Check the crate packages and builds as published    |
| `just man`         | Generate the `wullie(1)` man page in `target/man/`  |
| `just completions` | Generate shell completions in `target/completions/` |
| `just tarball`     | Build a release tarball for this machine in `dist/` |
| `just deb`         | Build a Debian package for this machine in `dist/`  |
| `just site`        | Build the website in `target/site/`                 |
| `just website`     | Publish the website (needs the deploy key)          |

Pages in `website/` can use `@VERSION@`, which `just site` replaces with the crate's version.

## Licensing

This repository is [REUSE][reuse]-compliant.
Every new file needs an SPDX header, for example:

```rust
// SPDX-FileCopyrightText: 2026 Your Name <you@example.com>
// SPDX-License-Identifier: GPL-3.0-or-later
```

Files that can't hold a comment (such as `Cargo.lock`) need an entry in `REUSE.toml` instead.
`just reuse` checks this.

## Dependency Output

Wullie represents dependency declarations as one list without separating normal, development, build, optional, workspace or target-specific dependencies.
Development dependencies can be an attack path, and optional dependencies may receive less testing while still affecting live deployments.
Ecosystems also divide dependencies differently, so the shared dependency model does not encode those categories.

Malformed dependency sections or entries produce a warning, and Wullie skips the entire manifest rather than reporting a partial dependency list.

## Version Requirements

The Update Finder treats a dependency as outdated when the lowest published, non-yanked release matching its requirement is older than the newest non-yanked release.
It does not treat a range as current just because that range also permits the latest release.
Pre-releases and Python development releases are ignored unless the requirement names a pre-release or development release.
Deprecated npm versions count as yanked, since the author has asked that they no longer be used.
Semver and PEP 440 ranges are convenient during development, but they can admit untested or vulnerable in-between releases.
A fully specified exact requirement such as Cargo's `=1.2.3`, Python's `==1.2.3` or npm's `1.2.3` is the most robust choice from an SBOM perspective because it identifies one version rather than a range.

Across every ecosystem, an unspecified version is out of date.
Cargo's `*` matches every stable release, so its lowest matching release is the oldest one, and it is reported until it is pinned.
A bare Python name has an empty requirement and matches every release, so it is reported until pinned.
An npm `*`, `x` or empty requirement is treated the same way, and so is a dist-tag such as `latest`, because a tag names whatever version it points to today rather than a version.

Requirements are compared using each ecosystem's own rules, so the same string can mean different things.
A bare `1.2.3` is a caret requirement in Cargo but an exact version in npm.

## Diff Maker

The Diff Maker prints one combined unified diff to stdout.
The paths in the diff are relative to the starting `PATH`.
The diff on stdout contains every change across every project, so stderr carries our warnings and errors.

The Diff Maker replaces every range (caret, tilde, wildcard, comparison, dist-tag or bare name) with an exact pin of the newest non-yanked release: `=x.y.z` for Cargo, `==x.y.z` for Python, and a bare `x.y.z` for npm.
It pins to the newest release even when that crosses a major version, and drops other bounds and exclusions such as `<3` or `!=2.1.0`.
Cargo pins drop any build metadata, so `0.25.20+spec-1.1.0` is pinned as `=0.25.20`, because Cargo ignores and warns about metadata in requirements and crates.io never publishes two versions that differ only by it.

The Diff Maker never updates lock files, so regenerate `Cargo.lock`, `uv.lock`, `package-lock.json` or the equivalent after applying a diff.

## Registry

`Registry` is a trait because registry clients are an open set, including crates.io, PyPI and npm whilst also letting tests use a fake registry without making network requests.
`Ecosystem` remains an enum because the supported ecosystems are a closed set, and exhaustive matches ensure each ecosystem handles every pipeline stage.

### Timeouts and Retries

There are timeout limits on each step: 10 seconds to connect, and 30 seconds for each partial read, with a backstop of 5 minutes for the whole connection.
A slow but steady download of a large index keeps going, while a stalled connection fails.

A missing package or any other client error fails at once.

A lookup that fails in a way that is likely to be temporary is tried up to 3 times.
Wullie waits up to 10 seconds; if a registry asks for longer, through a `Retry-After` header, its remaining lookups fail at once with a warning.
A registry that can't be reached at all, because its name doesn't resolve or connecting to it fails even after retrying, isn't sent anything more during that run.
The first failure is reported as usual, and the lookups skipped after it are reported in one warning per registry, such as `skipped 212 more lookups on index.crates.io because it couldn't be reached`.
Lookups skipped because a registry asked, with `Retry-After`, for a longer wait are summed the same way, such as `skipped 40 more lookups on registry.npmjs.org because it asked not to be sent requests yet`.

Lookups are made one at a time, never in parallel.
We deliberately don't fetch concurrently: crates.io, PyPI and npm are free, shared services, and a burst of requests from one run isn't a friendly way to use them, even with the back-off and `Retry-After` handling above.

### Lookup Cache

Raw index bodies are stored under `<cache directory>/<ecosystem>/<index path>`.
The cache directory is each platform's usual one: `$XDG_CACHE_HOME/wullie` on Linux, `~/Library/Caches/xyz.mikecoats.wullie` on macOS and `%LOCALAPPDATA%\MikeCoats\wullie\cache` on Windows.
On macOS, a set `$XDG_CACHE_HOME` wins, for people who already keep their caches there; otherwise `~/Library/Caches` keeps the cache out of Time Machine backups and lets macOS clear it when space runs low.
Each part of the path keeps is formatted such that there are no collisions on case-insensitive filesystems.
An entry is fresh for a fixed 4 hours from its file modification time, and `--refresh` treats every entry as stale.
We deliberately don't revalidate with `ETag` or `Last-Modified`: a fixed lifetime is simpler and `--refresh` covers the times freshness matters.
Each run first deletes entries older than 4 hours, along with any directories left empty, so the cache only holds recent lookups.
Pruning always uses the 4-hour lifetime, even with `--refresh`, so a refresh of one tree doesn't empty the cache for every other tree.

## Crate Layout

Wullie ships as a binary, not a library.
The pipeline's modules belong to the binary crate, declared in `src/main.rs`.
The library crate holds only `cli`, so `xtask` can render the man page and completions from the same definitions as `--help`.
Only a change to `cli` can break anything outside the binary, so other changes don't need a new minor version.

## Errors

Each stage uses `thiserror` for typed errors, and wraps errors from its dependencies in its own opaque types.
Starting up the command uses `anyhow` to add context to its errors.

## Untrusted Input

The tree being searched may hold files from anyone, so Wullie treats manifests as untrusted.

- Manifests larger than 16 MiB are skipped with a warning rather than read into memory.
- A leading UTF-8 byte-order mark is accepted in every manifest and kept in the diff.
- Everything written to stderr has its control characters escaped, other than tabs and newlines, and every line after the first is indented, so text from a manifest can't drive the terminal or pass as another warning.
  The diff on stdout is left exactly as the manifests are, because `git apply` and `patch` need it unchanged.

## Pruning

Wullie prunes the union of every ecosystem's build, cache and dependency directory names throughout the tree.
This is intentional: it avoids traversing large generated or vendored trees and prevents unrelated ecosystem manifests inside them from being reported.
A directory is pruned by name even when it is not actually used for that ecosystem.
The starting directory itself is not pruned, so searching from a directory with one of these names still works.

Symbolic links inside the tree are skipped without a warning, whether they point to a manifest or a directory.
Following them could leave the tree, loop forever, or produce a diff for a file that lives outside the tree.

## Exit Codes

- **We follow `ripgrep`.**
  It is a well-known search tool with the same shape as Wullie: walk a tree, print results, warn about problems along the way.
- **Warnings exit with `2`, even when projects were listed.**
  A warning means part of the tree couldn't be searched, so the list may be incomplete.
  A script should be able to tell a complete answer from a partial one without parsing stderr.
- **Cache problems are notes, and exit with `0`.**
  If a lookup can't be cached, or there is no cache directory, the answer is still complete, just slower next time.
  These are printed as `Note:` rather than `Warning:`, so the exit status keeps meaning "complete or not".
- **Finding nothing exits with `0`.**
  This is where we differ from `ripgrep`, which exits with `1` when nothing matches.
  An empty tree is a valid, complete answer for Wullie, not a failed search.

## Lints

Some of our CI GitHub Actions were failing due to more restrictive linting rules.
We've now enabled some lints in `Cargo.toml` to turn on clippy's `pedantic` group and a few restriction lints, such as `unwrap_used` and `print_stdout`.
`clippy.toml` exempts tests from the restriction lints.
When an exception is needed, use `#[expect(lint, reason = "...")]` rather than `#[allow]`, so the reason is recorded and the attribute fails once it's no longer needed.

## Before Submitting

Make sure `just ci` passes.

## Releasing

Pushing a `v*` tag starts the release workflow in `.github/workflows/release.yml`.

1. Bump `version` in `Cargo.toml`.
2. Run `just ci`, which also updates `Cargo.lock` to the new version.
3. Commit `Cargo.toml` and `Cargo.lock` together, then push `main`.
4. Tag the commit with a signed tag, named for the version prefixed by `v`, then push the tag:

   ```bash
   git tag -s v0.1.1
   git push origin v0.1.1
   ```

   The tag message becomes the top of the GitHub release notes, followed by notes GitHub generates from the changes since the previous release.

The tag must be `v` followed by the exact version in `Cargo.toml`.
A tag without the `v` doesn't start the workflow, and a mismatched version fails it.
It publishes the crate to [crates.io][crates-io] and attaches prebuilt tarballs to a GitHub release, which [`cargo binstall`][binstall] downloads.
The Linux x64 and ARM builds are also packaged as `.deb` files for `amd64` and `arm64`, which need glibc 2.34 or newer, so they install on Debian 12 and later.

If the version check fails, delete the tag, fix the version, commit, and tag again:

```bash
git tag -d v0.1.1
git push origin :refs/tags/v0.1.1
```

If a later step fails, a draft release may already exist.
Delete it with `gh release delete v0.1.1` before pushing the tag again, or re-run the failed jobs from the Actions page if nothing needs to change.

A version published to crates.io can be yanked but never replaced, so a broken release needs a new patch version.

## Rust Version

We predominantly ship releases as prebuilt binaries, so Wullie just targets one recent Rust version.
`Cargo.toml`s and `rust-toolchain.toml` pin the version used locally, in CI and for releases, and `rustup` installs it automatically.

To update, change `channel` in `rust-toolchain.toml` and `rust-version` in both `Cargo.toml` and `xtask/Cargo.toml`, then fix anything `just ci` reports.

## Writing Markdown

Every sentence should be written on its own line.
Rendered HTML joins the lines back into paragraphs, and diffs then show exactly which sentence changed.
Reference-style links are preferred to inline links.

[rust]: https://rustup.rs/
[just]: https://just.systems/
[reuse]: https://reuse.software/
[shellcheck]: https://www.shellcheck.net/
[crates-io]: https://crates.io/crates/wullie
[binstall]: https://github.com/cargo-bins/cargo-binstall
