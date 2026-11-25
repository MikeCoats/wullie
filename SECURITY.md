<!--
SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Security Policy

## Supported Versions

Only the latest release receives security fixes.
Fixes ship as a new patch release, and earlier versions are not patched.

## Reporting a Vulnerability

Please don't report vulnerabilities in public issues, pull requests or discussions.
Report them privately through GitHub's private vulnerability reporting instead.
The report stays private between us until a fix is released.

A useful report includes:

- the Wullie version, from `wullie --version`, and your operating system
- steps to reproduce the problem, ideally with a minimal manifest or directory tree
- what an attacker could achieve
- whether, and how, you would like to be credited

## What to Expect

Wullie is maintained by one person in their spare time, so these are aims rather than guarantees.

- You'll get an acknowledgement within 7 days.
- You'll get an assessment of whether the report is accepted within 14 days.
- Accepted vulnerabilities will be fixed in a new release as soon as practical, and within 90 days of the report.

Once a fix is released, a GitHub security advisory is published, and the reporter is credited unless they would rather not be.
If a fix takes longer than 90 days, we'll agree a disclosure date with you rather than keep the report private indefinitely.

## Scope

Wullie reads files it didn't write and data from the network, so most issues fall in one of these areas.

### In Scope

- Untrusted manifests.
  
  Wullie parses `Cargo.toml`, `pyproject.toml`, `package.json` and related files anywhere in the tree it searches.
  A crafted file that causes a crash, a hang, excessive memory use, or anything worse is in scope.

- Registry responses.
  
  Wullie trusts index data from crates.io and PyPI.
  A malicious or tampered response that leads to unsafe behaviour or a misleading diff is in scope.

- Terminal output.
  
  Warnings include names, requirements and paths from the searched tree.
  A way to get raw control characters onto the terminal through them is in scope.
  The diff on stdout reproduces manifest lines exactly, as `git diff` does, so their contents are out of scope there.

- The look-up cache.
  
  Index bodies are written under `$XDG_CACHE_HOME/wullie/`.
  Writing outside that directory, for example through a crafted package name, or poisoning the cache, is in scope.

- The Diff Maker.
  
  Its output is meant to be applied with `git apply`.
  A diff that touches files outside the searched tree, or pins a different version from the one reported, is in scope.

- Releases.
  
  Problems with the published crate, the release tarballs, their checksums, or the workflows that build them are in scope.

### Out of Scope

- Vulnerabilities in Wullie's dependencies.

  Please report these to the dependency's maintainers.
  If a dependency's vulnerability is exploitable through Wullie, that is in scope here too.

- Vulnerable packages that Wullie reports as up to date.

  Wullie compares versions with the newest release, and it doesn't check security advisories.

- Malicious releases on a registry.

  Wullie pins the newest release it finds, and it can't tell whether that release is trustworthy.
  Review every diff, and the releases it pins, before you apply it.
