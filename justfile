# SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# List available recipes
default:
    @just --list

# Run every check that CI runs
ci: fmt lint test msrv reuse package man completions

# Check formatting
fmt:
    cargo fmt --all --check

# Lint with clippy, treating warnings as errors
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run the test suite
test:
    cargo test --workspace

# Check the build with the minimum supported Rust version
msrv:
    cargo +1.88 check --workspace --all-targets

# Check REUSE licensing compliance
reuse:
    reuse lint

# Check the crate packages and builds as published
package:
    cargo package --allow-dirty

# Generate the wullie(1) man page in target/man/
man:
    cargo xtask man

# Generate shell completions in target/completions/
completions:
    cargo xtask completions

# Build a release tarball in dist/
dist target=`rustc -vV | sed -n 's/^host: //p'`:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(cargo pkgid | sed 's/.*[#@]//')
    name="wullie-${version}-{{target}}"
    cargo build --release --target {{target}}
    rm -rf "dist/${name}"
    mkdir -p "dist/${name}"
    cargo xtask man --out-dir "dist/${name}"
    cargo xtask completions --out-dir "dist/${name}/completions"
    cp "target/{{target}}/release/wullie" README.md "dist/${name}/"
    cp -r LICENSES "dist/${name}/"
    "${TAR:-tar}" --sort=name --owner=0 --group=0 --numeric-owner \
        --mtime="@$(git log -1 --format=%ct)" \
        -C dist -czf "dist/${name}.tar.gz" "${name}"
    (cd dist && shasum -a 256 "${name}.tar.gz" > "${name}.tar.gz.sha256")
    echo "dist/${name}.tar.gz"
