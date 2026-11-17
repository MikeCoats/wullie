<!--
SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Wullie

> Yer local groundskeeper

## Usage

Run `wullie [PATH]` to list dependency version requirements under `PATH`.
If omitted, Wullie uses the current working directory.

Wullie recursively finds:

- Rust projects, by their `Cargo.toml` files.
- Python projects, by their `pyproject.toml` files.
- JavaScript projects, by their `package.json` files.

A directory with manifests for more than one ecosystem is listed once per ecosystem.

## Output

Wullie prints one tab-separated line per dependency with a version requirement.
Dependencies are reported as one list rather than being classified by ecosystem-specific categories.
A project with no dependency version requirements produces no output.

Results respect ignore files and hidden directories, and skip `target/`, `__pycache__/`, `__pypackages__/`, `site-packages/`, `node_modules/` and `bower_components/` directories.
Walk and manifest problems are printed to stderr as warnings before dependency output, and the search carries on.

Wullie exits with:

- `0` when the search completed without problems, whether or not any projects were found;
- `2` when anything went wrong, including warnings, an invalid `PATH`, bad arguments or a failure to write the output, even if some projects were listed.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to build, test and lint Wullie.

## Licensing

This repository is REUSE-compliant.
The licensing of each file can be divined with `reuse spdx` or `reuse lint`, but here is a best-effort summary:

- All original source code is licensed under GPL-3.0-or-later.

Contributors: see [CONTRIBUTING.md](CONTRIBUTING.md#licensing) for how to licence new files.
