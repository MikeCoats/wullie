<!--
SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Wullie

> Yer local groundskeeper

## Usage

Run `wullie [PATH]` to find out-of-date dependency requirements under `PATH`.
If omitted, Wullie uses the current working directory.

Wullie recursively searches for supported project manifests:

- Rust projects, by their `Cargo.toml` files.
- Python projects, by their `pyproject.toml` files.
- JavaScript projects, by their `package.json` files.

A directory with manifests for more than one ecosystem is found once per ecosystem.
Dependency update checking is currently implemented for Rust; Python and JavaScript manifests produce no dependency results yet.

## Output

Wullie prints only out-of-date dependencies, one tab-separated line per dependency.
Each line contains `<ecosystem>\t<project directory>\t<name>\t<requirement>\t<lowest>\t<latest>`.
The requirement is printed exactly as written in the manifest.
`<lowest>` is the lowest published, non-yanked release matching the requirement, and `<latest>` is the newest non-yanked release.
Pre-releases are excluded unless the requirement names a pre-release.
A dependency is out of date when `<lowest>` is older than `<latest>`, even if its requirement also permits the newest release.
A project with no out-of-date dependencies produces no output.
Rust dependencies that specify `registry` or `registry-index` are omitted, so private packages are not compared with same-named crates.io packages.
Rust update checks contact the crates.io sparse index at `https://index.crates.io`.

Results respect ignore files and hidden directories, and skip `target/`, `__pycache__/`, `__pypackages__/`, `site-packages/`, `node_modules/` and `bower_components/` directories.
Walk, manifest, lookup and comparison problems are printed to stderr as warnings before update output, and the search carries on.

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
