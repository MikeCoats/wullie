<!--
SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Wullie

> Yer local groundskeeper

`wullie` is an opinionated tool for managing dependency requirements in Rust, Python, and JavaScript projects.

`wullie` believes that `>`, `^`, and `~` are all useful signifiers for specifying dependencies during development, but for maintenance of live, deployed, projects there is opportunity for a security gap.
_In theory_, npm, cargo and pip will automatically install the most recent specified package, hopefully picking up any security updates, but in practice these often don't make it through to production in a timely manner.
Lockfiles seek to address this gap, but hide it behind separate commands or sources, making it possible to accidentally install insecure versions.

`wullie` instead makes dependency management an intentional task, but provides a simple way to keep dependencies up-to-date.

## Usage

Run `wullie [PATH]` to find out-of-date dependencies and generate a `git apply`-ible patch with the latest updates.

Applying a dependency diff does not install and update packages or update lock files; remember to regenerate and recommit `Cargo.lock`, `uv.lock`, `package-lock.json` or the equivalent afterward.

Wullie recursively searches for supported project manifests:

- Rust projects, by their `Cargo.toml` files.
- Python projects, by their `pyproject.toml` files.
- JavaScript projects, by their `package.json` files.

A directory with manifests for more than one ecosystem is found once per ecosystem.

To save potentially thousands of duplicate network calls `wullie` caches its lookups for four hours.
If you need to fetch a fresh dependency for a recent security update, run `wullie --refresh [PATH]` to ignore cached lookups, fetch them again and update the cache.

## Nerdy Details

For Rust packages, `wullie` checks the crates.io sparse index at `https://index.crates.io`.
Rust dependencies that specify `registry`, `registry-index`, `path` or `git`, or are inherited with `workspace = true`, are omitted, so private and local packages are not compared with same-named crates.io packages.

For Python packages, `wullie` checks the PyPI Simple API at `https://pypi.org/simple`.
Python dependencies with declared extras and environment markers are retained.

For JavaScript packages, `wullie` checks the npm registry at `https://registry.npmjs.org`.
JavaScript dependencies on local paths, git repositories, URLs or workspaces are omitted, and `npm:` aliases are checked under the package they point to.

`wullie`'s scanner respects standard ignore files and hidden directories, and skips `target/`, `__pycache__/`, `__pypackages__/`, `site-packages/`, `node_modules/` and `bower_components/` directories.

## Contributing

See [CONTRIBUTING.md][contrib] for how to build, test and lint Wullie.

## Licensing

This repository is REUSE-compliant.
The licensing of each file can be divined with `reuse spdx` or `reuse lint`, but here is a best-effort summary:

- All original source code is licensed under GPL-3.0-or-later.

Contributors: see [CONTRIBUTING.md][licensing] for how to licence new files.

[contrib]: ./CONTRIBUTING.md
[licensing]: ./CONTRIBUTING.md#licensing
