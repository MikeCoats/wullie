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

## Output

Wullie prints only out-of-date dependencies, one tab-separated line per dependency.
Each line contains `<ecosystem>\t<project directory>\t<name>\t<requirement>\t<lowest>\t<latest>`.
`<lowest>` is the lowest published, non-yanked release matching the requirement, and `<latest>` is the newest non-yanked release.
A dependency is out of date when `<lowest>` is older than `<latest>`, even if its requirement also permits the newest release.
A project with no out-of-date dependencies produces no output.
Rust dependencies that specify `registry` or `registry-index` are omitted, so private packages are not compared with same-named crates.io packages.
Rust update checks contact the crates.io sparse index at `https://index.crates.io`.
Python update checks contact the PyPI Simple API at `https://pypi.org/simple`.

Results respect ignore files and hidden directories, and skip `target/`, `__pycache__/`, `__pypackages__/`, `site-packages/`, `node_modules/` and `bower_components/` directories.
Walk, manifest, lookup and comparison problems are printed to stderr as warnings before update output, and the search carries on.

Wullie exits with:

- `0` when the search results are complete, whether or not any projects were found; cache warnings do not change this;
- `2` when searching or writing output encountered a problem, including incomplete-result warnings, an invalid `PATH`, bad arguments or a failure to write the output.

## Lookup cache

Registry lookups are cached in `$XDG_CACHE_HOME/wullie/` (usually `~/.cache/wullie/`).
A cached lookup is reused for 4 hours, so releases published or yanked within that time may not be seen until it expires.
Run `wullie --refresh [PATH]` to ignore cached lookups, fetch them again and update the cache.
If the cache can't be read or written, Wullie fetches from the registry instead.
If Wullie cannot determine a user cache directory, it warns that lookups will not be cached, but still completes normally.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to build, test and lint Wullie.

## Licensing

This repository is REUSE-compliant.
The licensing of each file can be divined with `reuse spdx` or `reuse lint`, but here is a best-effort summary:

- All original source code is licensed under GPL-3.0-or-later.

Contributors: see [CONTRIBUTING.md](CONTRIBUTING.md#licensing) for how to licence new files.
