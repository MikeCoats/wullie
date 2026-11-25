# SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# List available recipes
default:
    @just --list

# Run every check that CI runs
ci: fmt lint test reuse package man completions

# Check formatting
fmt:
    cargo fmt --all --check

# Lint with clippy, treating warnings as errors
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run the test suite
test:
    cargo test --workspace

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
    exe=""
    if [[ "{{target}}" == *-windows-* ]]; then exe=".exe"; fi
    cargo build --release --target {{target}}
    rm -rf "dist/${name}"
    mkdir -p "dist/${name}"
    cargo xtask man --out-dir "dist/${name}"
    cargo xtask completions --out-dir "dist/${name}/completions"
    cp "target/{{target}}/release/wullie${exe}" README.md "dist/${name}/"
    cp -r LICENSES "dist/${name}/"
    "${TAR:-tar}" --sort=name --owner=0 --group=0 --numeric-owner \
        --mtime="@$(git log -1 --format=%ct)" \
        -C dist -czf "dist/${name}.tar.gz" "${name}"
    # Git for Windows has `sha256sum` but may not have `shasum`; macOS may be
    # the other way round. Both write the same format.
    if command -v sha256sum > /dev/null; then sum=(sha256sum); else sum=(shasum -a 256); fi
    (cd dist && "${sum[@]}" "${name}.tar.gz" > "${name}.tar.gz.sha256")
    echo "dist/${name}.tar.gz"
