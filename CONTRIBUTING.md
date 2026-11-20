<!--
SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Contributing to Wullie

## Prerequisites

- [Rust][rust] 1.88 or newer
- [`just`][just]: e.g. `cargo install just`, or `cargo binstall just` for a prebuilt binary
- [`reuse`][reuse] e.g. `pipx install reuse`

## Common Recipes

| Command        | What it does                                        |
|----------------|-----------------------------------------------------|
| `just`         | List the available recipes                          |
| `just ci`      | Run every check that CI runs                        |
| `just fmt`     | Check formatting with `cargo fmt --check`           |
| `just lint`    | Lint with clippy, treating warnings as errors       |
| `just test`    | Run the test suite                                  |
| `just msrv`    | Check the build with Rust 1.88, the minimum version |
| `just reuse`   | Check REUSE licensing compliance with `reuse lint`  |
| `just package` | Check the crate packages and builds as published    |
| `just dist`    | Build a release tarball for this machine in `dist/` |

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

## Version Requirements

The Update Finder treats a dependency as outdated when the lowest published, non-yanked release matching its requirement is older than the newest non-yanked release.
It does not treat a range as current just because that range also permits the latest release.
Pre-releases and Python development releases are ignored unless the requirement names a pre-release or development release.
Semver and PEP 440 ranges are convenient during development, but they can admit untested or vulnerable in-between releases.
A fully specified exact requirement such as Cargo's `=1.2.3` or Python's `==1.2.3` is the most robust choice from an SBOM perspective because it identifies one version rather than a range.

Across every ecosystem, an unspecified version is out of date.
Cargo's `*` matches every stable release, so its lowest matching release is the oldest one, and it is reported until it is pinned.
A bare Python name has an empty requirement and matches every release, so it is reported until pinned.

## Diff Maker

The Diff Maker prints one combined unified diff to stdout.
The paths in the diff are relative to the starting `PATH`.
The diff already every change across every project, so stderr carries only warnings and errors.

The Diff Maker replaces every range (caret, tilde, wildcard, comparison or bare name) with an exact pin of the newest non-yanked release: `=x.y.z` for Cargo, `==x.y.z` for Python.
For Python requirements, extras, environment markers and surrounding TOML comments are retained when the version specifier is replaced.
It pins to the newest release even when that crosses a major version, and drops other bounds and exclusions such as `<3` or `!=2.1.0`.
The Diff Maker never updates lock files, so regenerate `Cargo.lock`, `uv.lock` or the equivalent after applying a diff.

## Registry

`Registry` is a trait because registry clients are an open set, including crates.io and PyPI now and possible JavaScript registries later.
The trait also lets tests use a fake registry without making network requests.
`Ecosystem` remains an enum because the supported ecosystems are a closed set, and exhaustive matches ensure each ecosystem handles every pipeline stage.

### Look-up Cache

Raw index bodies are stored under `$XDG_CACHE_HOME/wullie/<ecosystem>/<index path>`.
An entry is fresh for a fixed 4 hours from its file modification time, and `--refresh` treats every entry as stale.
We deliberately don't revalidate with `ETag` or `Last-Modified`: a fixed lifetime is simpler and `--refresh` covers the times freshness matters.

## Errors

The library uses `thiserror` for typed errors without exposing dependency-specific error types in its public API.
The CLI uses `anyhow` to add context to command-line errors.

## Pruning

Wullie prunes the union of every ecosystem's build, cache and dependency directory names throughout the tree.
This is intentional: it avoids traversing large generated or vendored trees and prevents unrelated ecosystem manifests inside them from being reported.
A directory is pruned by name even when it is not actually used for that ecosystem.
The starting directory itself is not pruned, so searching from a directory with one of these names still works.

## Exit Codes

- **We follow `ripgrep`.**
  It is a well-known search tool with the same shape as Wullie: walk a tree, print results, warn about problems along the way.
- **Warnings exit with `2`, even when projects were listed.**
  A warning means part of the tree couldn't be searched, so the list may be incomplete.
  A script should be able to tell a complete answer from a partial one without parsing stderr.
- **Finding nothing exits with `0`.**
  This is where we differ from `ripgrep`, which exits with `1` when nothing matches.
  An empty tree is a valid, complete answer for Wullie, not a failed search.

## Writing Markdown

Every sentence should be written on its own line.
Rendered HTML joins the lines back into paragraphs, and diffs then show exactly which sentence changed.
Reference-style links are preferred to inline links.

## Before Submitting

Make sure `just ci` passes.


[rust]: https://rustup.rs/
[just]: https://just.systems/
[reuse]: https://reuse.software/
