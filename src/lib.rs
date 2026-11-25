// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The command-line definition for Wullie.
//!
//! Wullie ships as a binary, and the whole pipeline lives in the `wullie`
//! binary crate. This library only holds [`cli`], so the binary's `--help` and
//! the man page and completions that `xtask` renders come from the same
//! definitions.

pub mod cli;
