#!/bin/sh
# SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# Adds Wullie's apt repository, https://wullie.mikecoats.xyz/apt/, on Debian
# 12 or later, or Ubuntu 22.04 or later.
#
#   curl -fsSL https://wullie.mikecoats.xyz/apt/setup.sh | sudo sh

set -eu

# Everything is in a function, so a download cut short runs nothing.
main() {
    # The key that signs the repository.
    install -d -m 755 /etc/apt/keyrings
    curl -fsSLo /etc/apt/keyrings/wullie.asc https://wullie.mikecoats.xyz/apt/wullie.asc
    chmod 644 /etc/apt/keyrings/wullie.asc

    # Where the repository is, and which key it's signed with.
    cat > /etc/apt/sources.list.d/wullie.sources <<'EOF'
Types: deb
URIs: https://wullie.mikecoats.xyz/apt/
Suites: stable
Components: main
Signed-By: /etc/apt/keyrings/wullie.asc
EOF

    echo "Added Wullie's apt repository. To install Wullie:"
    echo "  sudo apt update && sudo apt install wullie"
}

main
