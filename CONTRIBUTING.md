<!--
SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Contributing to Wullie

## Prerequisites

- [Rust](https://rustup.rs/) 1.85 or newer
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

## Writing Markdown

Every sentence should be written on its own line.
Rendered HTML joins the lines back into paragraphs, and diffs then show exactly which sentence changed.

## Before submitting

Make sure `just ci` passes.
