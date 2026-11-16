<!--
SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Wullie

> Yer local groundskeeper

## Usage

Run `wullie [PATH]` to search from `PATH`; if omitted, Wullie uses the current working directory.
Wullie recursively finds:

- Rust projects, by their `Cargo.toml` files.
- Python projects, by their `pyproject.toml` files.

A directory with manifests for more than one ecosystem is listed once per ecosystem.
Results respect ignore files and hidden directories, and skip `target/`, `__pycache__/`, `__pypackages__/` and `site-packages/` directories.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to build, test and lint Wullie.

## Licensing

This repository is REUSE-compliant.
The licensing of each file can be divined with `reuse spdx` or `reuse lint`, but here is a best-effort summary:

- All original source code is licensed under GPL-3.0-or-later.

Contributors: see [CONTRIBUTING.md](CONTRIBUTING.md#licensing) for how to licence new files.
