<!--
SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Contributing to Wullie

## Prerequisites

- [Rust](https://rustup.rs/) 1.88 or newer
- [`just`](https://just.systems/): `cargo install just`
- [`reuse`](https://reuse.software/): e.g. `pipx install reuse`

## Common commands

| Command      | What it does                                         |
|--------------|------------------------------------------------------|
| `just`       | List the available recipes                           |
| `just ci`    | Run every check that CI runs                         |
| `just fmt`   | Check formatting with `cargo fmt --check`            |
| `just lint`  | Lint with clippy, treating warnings as errors        |
| `just test`  | Run the test suite                                   |
| `just msrv`  | Check the build with Rust 1.88, the minimum version  |
| `just reuse` | Check REUSE licensing compliance with `reuse lint`   |

## Licensing

This repository is [REUSE](https://reuse.software/)-compliant.
Every new file needs an SPDX header, for example:

```rust
// SPDX-FileCopyrightText: 2026 Your Name <you@example.com>
// SPDX-License-Identifier: GPL-3.0-or-later
```

Files that can't hold a comment (such as `Cargo.lock`) need an entry in `REUSE.toml` instead.
`just reuse` checks this.

## Dependency output

Wullie reports versioned dependencies as one list without separating normal, development, build, optional, workspace or target-specific dependencies.
Development dependencies can be an attack path, and optional dependencies may receive less testing while still affecting live deployments.
Ecosystems also divide dependencies differently, so the shared dependency model does not encode those categories.

## Version requirements

The Rust Update Finder treats a dependency as outdated when the lowest published, non-yanked release matching its requirement is older than the newest non-yanked release.
It does not treat a range as current just because that range also permits the latest release.
Pre-releases are ignored unless the requirement names a pre-release.
Semver ranges are convenient during development, but they can admit untested or vulnerable in-between releases.
A fully specified exact requirement such as `=1.2.3` is the most robust choice from an SBOM perspective because it identifies one version rather than a range.

## Registry design

`Registry` is a trait because registry clients are an open set, including crates.io now and possible Python or JavaScript registries later.
The trait also lets tests use a fake registry without making network requests.
`Ecosystem` remains an enum because the supported ecosystems are a closed set, and exhaustive matches ensure each ecosystem handles every pipeline stage.

## Errors

The library uses `thiserror` for typed errors without exposing dependency-specific error types in its public API.
The CLI uses `anyhow` to add context to command-line errors.

## Directory pruning

Wullie prunes the union of every ecosystem's build, cache and dependency directory names throughout the tree.
This is intentional: it avoids traversing large generated or vendored trees and prevents unrelated ecosystem manifests inside them from being reported.
A directory is pruned by name even when it is not actually used for that ecosystem.
The starting directory itself is not pruned, so searching from a directory with one of these names still works.

## Exit codes

The README describes what each exit code means; this section explains why.
The logic lives in `exit_status` in `src/main.rs`.

- **We follow `ripgrep`.**
  It is a well-known search tool with the same shape as Wullie: walk a tree, print results, warn about problems along the way.
  Matching its conventions means scripts written for one behave sensibly with the other.
- **Warnings exit with `2`, even when projects were listed.**
  A warning means part of the tree couldn't be searched, so the list may be incomplete.
  A script should be able to tell a complete answer from a partial one without parsing stderr.
- **Every kind of failure uses `2`.**
  `clap` already exits with `2` for bad arguments, so invalid paths, warnings and output errors use the same code.
  Callers only need to check for one non-zero value.
- **Finding nothing exits with `0`.**
  This is where we differ from `ripgrep`, which exits with `1` when nothing matches.
  An empty tree is a valid, complete answer for Wullie, not a failed search.

## Writing Markdown

Every sentence should be written on its own line.
Rendered HTML joins the lines back into paragraphs, and diffs then show exactly which sentence changed.

## Before submitting

Make sure `just ci` passes.
