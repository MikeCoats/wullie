# SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# List available recipes
default:
    @just --list

# Run every check that CI runs
ci: fmt lint shellcheck test reuse package man completions

# Check formatting
fmt:
    cargo fmt --all --check

# Lint with clippy, treating warnings as errors
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Check the packaging scripts with shellcheck
shellcheck:
    shellcheck pkg/*/build

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
tarball target=`rustc -vV | sed -n 's/^host: //p'`:
    bash pkg/tarball/build {{target}}

# Build a Debian package in dist/ from the release tarball's contents
deb target=`rustc -vV | sed -n 's/^host: //p'`: (tarball target)
    bash pkg/debian/build {{target}}

# Build the website in target/site/, with this version filled in
site:
    rm -rf target/site
    cp -r website target/site
    find target/site -name '*.html' -exec sed -i "s/@VERSION@/$(cargo pkgid | sed 's/.*[#@]//')/g" {} +

# Publish the website to wullie.mikecoats.xyz, using the deploy key
website: site
    rsync -rlpt --delete --chmod=D755,F644 -e 'ssh -i ~/.ssh/wullie.website.id_ed25519' target/site/ wullie@wullie.mikecoats.xyz:
