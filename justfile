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
    shellcheck pkg/*/build website/apt/setup.sh

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
    mkdir -p target
    cp -r website target/site
    find target/site -name '*.html' -exec sed -i "s/@VERSION@/$(cargo pkgid | sed 's/.*[#@]//')/g" {} +
    # Show the apt setup script on its page, exactly as it's served.
    sed -e 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g' -e '1s/^/<pre><code>/' -e '$s|$|</code></pre>|' \
        website/apt/setup.sh > target/setup.sh.html
    sed -i -e '/^@SETUP_SH@$/{r target/setup.sh.html' -e 'd}' target/site/apt/index.html

# Build the signed apt repository in target/site/apt/ from the release's packages
apt: site
    gh release download "v$(cargo pkgid | sed 's/.*[#@]//')" --pattern '*.deb' --dir dist --clobber
    bash pkg/apt/build

# Publish the website and apt repository to wullie.mikecoats.xyz, using the deploy key
website: apt
    rsync -rlpt --delete --chmod=D755,F644 -e 'ssh -i ~/.ssh/wullie.website.id_ed25519' target/site/ wullie@wullie.mikecoats.xyz:
