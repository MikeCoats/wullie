# SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# List available recipes
default:
    @just --list

# Run every check that CI runs
ci: fmt lint test msrv reuse

# Check formatting
fmt:
    cargo fmt --check

# Lint with clippy, treating warnings as errors
lint:
    cargo clippy --all-targets -- -D warnings

# Run the test suite
test:
    cargo test

# Check the build with the minimum supported Rust version
msrv:
    cargo +1.88 check --all-targets

# Check REUSE licensing compliance
reuse:
    reuse lint
