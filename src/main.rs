// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `wullie` command: finds out-of-date dependencies under a directory and
//! prints a diff that pins them.
//!
//! The work happens in a pipeline of stages, each in its own module:
//!
//! 1. The Project Finder (`project`) walks a tree for manifests.
//! 2. The Dependency Finder (`dependency`) reads each manifest's dependencies.
//! 3. The Update Finder (`update`) compares them with their registries.
//! 4. The Diff Maker (`diff`) pins the outdated ones and builds the diff.
//!
//! The command-line definition lives in the library crate instead, so `xtask`
//! can render the man page and completions from it.

mod dependency;
mod diff;
mod project;
mod registry;
mod update;
mod walk;

use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, BufWriter, ErrorKind, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use wullie::cli::Cli;

use crate::dependency::find_dependencies;
use crate::diff::manifest_diffs;
use crate::project::find_projects;
use crate::registry::{Cache, HttpRegistry, MAX_AGE};
use crate::update::find_updates;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let root = match start_path(cli.path) {
        Ok(path) => path,
        Err(error) => {
            write_stderr(format!("Error: {error:#}"));
            return ExitCode::from(2);
        }
    };

    let project_findings = find_projects(&root);
    let dependency_findings = find_dependencies(&project_findings.projects);
    let (cache, cache_unavailable) = cache(cli.refresh);
    if let Some(cache) = &cache {
        cache.prune(MAX_AGE);
    }
    let registry = HttpRegistry::new(cache);
    let update_findings = find_updates(&dependency_findings.projects, &registry);
    let diff_findings = manifest_diffs(&root, &update_findings.projects);

    for error in &project_findings.errors {
        warn(error);
    }
    for error in &dependency_findings.errors {
        warn(error);
    }
    for error in &update_findings.errors {
        warn(error);
    }
    for error in &diff_findings.errors {
        warn(error);
    }
    // Cache problems are notes rather than warnings: the results are still
    // complete, so they don't change the exit status.
    if let Some(error) = registry.cache_write_error() {
        write_stderr(format!("Note: {}", chain(error)));
    }
    if cache_unavailable {
        write_stderr(
            "Note: cannot determine user cache directory; registry lookups will not be cached",
        );
    }

    let output = write_diff(io::stdout().lock(), &diff_findings.diff);
    if let Err(error) = &output
        && error.kind() != ErrorKind::BrokenPipe
    {
        write_stderr(format!("Error: cannot write output: {error}"));
    }

    let had_warnings = !project_findings.errors.is_empty()
        || !dependency_findings.errors.is_empty()
        || !update_findings.errors.is_empty()
        || !diff_findings.errors.is_empty();
    ExitCode::from(exit_status(had_warnings, &output))
}

/// The user's registry cache; `--refresh` makes every entry stale, so lookups
/// are fetched again and rewritten.
/// Without a cache directory, lookups simply aren't cached.
fn cache(refresh: bool) -> (Option<Cache>, bool) {
    let Some(dir) = Cache::user_dir() else {
        return (None, true);
    };
    let max_age = if refresh { Duration::ZERO } else { MAX_AGE };
    (Some(Cache::new(dir, max_age)), false)
}

/// Exit status, following ripgrep: 2 if anything went wrong, even if some
/// results were printed, otherwise 0.
///
/// A closed pipe (e.g. `wullie | head -1`) isn't an error: the reader has all
/// it wants.
fn exit_status(had_warnings: bool, output: &io::Result<()>) -> u8 {
    let output_failed = output
        .as_ref()
        .is_err_and(|error| error.kind() != ErrorKind::BrokenPipe);
    if had_warnings || output_failed { 2 } else { 0 }
}

fn write_diff(output: impl Write, diff: &str) -> io::Result<()> {
    if diff.is_empty() {
        return Ok(());
    }
    let mut output = BufWriter::new(output);
    output.write_all(diff.as_bytes())?;
    output.flush()
}

fn warn(error: &dyn Error) {
    write_stderr(format!("Warning: {}", chain(error)));
}

/// An error's message followed by each of its causes, separated by `: `, like
/// `anyhow`'s `{:#}`.
fn chain(error: &dyn Error) -> String {
    let mut message = error.to_string().trim_end().to_owned();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(cause.to_string().trim_end());
        source = cause.source();
    }
    message
}

fn write_stderr(message: impl std::fmt::Display) {
    let message = for_terminal(&message.to_string());
    let _ = writeln!(io::stderr().lock(), "{message}");
}

/// Makes a message safe to print to a terminal.
///
/// Messages hold names, requirements and paths from the tree being searched,
/// which may be untrusted. Left raw, control characters could drive the
/// terminal: retitle it, erase lines, or worse in some terminals. So every
/// control character except tab and newline is written as a Rust escape such
/// as `\u{1b}`, and each line after the first is indented, so a newline can't
/// start a line that passes as another `Warning:` or `Error:`.
fn for_terminal(message: &str) -> String {
    let mut safe = String::with_capacity(message.len());
    for character in message.chars() {
        match character {
            '\n' => safe.push_str("\n  "),
            '\t' => safe.push('\t'),
            character if character.is_control() => safe.extend(character.escape_debug()),
            character => safe.push(character),
        }
    }
    safe
}

fn start_path(arg: Option<PathBuf>) -> Result<PathBuf> {
    let path = match arg {
        Some(path) => path,
        None => env::current_dir().context("could not determine current directory")?,
    };

    let path =
        fs::canonicalize(&path).with_context(|| format!("cannot access {}", path.display()))?;

    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{chain, exit_status, for_terminal, start_path, write_diff};
    use std::io::{self, ErrorKind, Write};
    use std::{env, fs, path::PathBuf};

    struct ClosedPipe;

    impl Write for ClosedPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn writes_diff_unchanged() {
        let mut output = Vec::new();
        write_diff(&mut output, "--- a/Cargo.toml\n+++ b/Cargo.toml\n").unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "--- a/Cargo.toml\n+++ b/Cargo.toml\n"
        );
    }

    #[test]
    fn closed_diff_output_returns_broken_pipe_instead_of_panicking() {
        let error = write_diff(ClosedPipe, "--- a/Cargo.toml\n").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::BrokenPipe);
    }

    #[test]
    fn clean_run_exits_zero() {
        assert_eq!(exit_status(false, &Ok(())), 0);
    }

    #[test]
    fn warnings_exit_two() {
        assert_eq!(exit_status(true, &Ok(())), 2);
    }

    #[test]
    fn broken_pipe_is_not_an_error() {
        assert_eq!(exit_status(false, &Err(ErrorKind::BrokenPipe.into())), 0);
        assert_eq!(exit_status(true, &Err(ErrorKind::BrokenPipe.into())), 2);
    }

    #[test]
    fn other_output_errors_exit_two() {
        assert_eq!(exit_status(false, &Err(ErrorKind::Other.into())), 2);
    }

    #[derive(Debug, thiserror::Error)]
    #[error("outer")]
    struct Outer(#[source] Middle);

    #[derive(Debug, thiserror::Error)]
    #[error("middle")]
    struct Middle(#[source] io::Error);

    #[test]
    fn chain_includes_every_cause() {
        let error = Outer(Middle(io::Error::other("inner")));
        assert_eq!(chain(&error), "outer: middle: inner");
    }

    #[test]
    fn chain_trims_trailing_newlines_from_multi_line_messages() {
        let error = Middle(io::Error::other("line one\nline two\n"));
        assert_eq!(chain(&error), "middle: line one\nline two");
    }

    #[test]
    fn for_terminal_escapes_control_characters() {
        assert_eq!(
            for_terminal("cannot look up \u{1b}]0;title\u{7}\u{1b}[2Kevil"),
            "cannot look up \\u{1b}]0;title\\u{7}\\u{1b}[2Kevil"
        );
        assert_eq!(for_terminal("\r\u{7f}\u{9b}"), "\\r\\u{7f}\\u{9b}");
    }

    #[test]
    fn for_terminal_keeps_tabs_and_ordinary_text() {
        assert_eq!(for_terminal("caf\u{e9}\tok"), "caf\u{e9}\tok");
    }

    #[test]
    fn for_terminal_indents_continuation_lines() {
        assert_eq!(
            for_terminal("Warning: one\nWarning: forged"),
            "Warning: one\n  Warning: forged"
        );
    }

    #[test]
    fn chain_of_an_error_without_a_source_is_its_message() {
        assert_eq!(chain(&io::Error::other("alone")), "alone");
    }

    #[test]
    fn missing_path_uses_current_directory() {
        let current_dir = env::current_dir().unwrap();
        assert_eq!(
            start_path(None).unwrap(),
            fs::canonicalize(current_dir).unwrap()
        );
    }

    #[test]
    fn provided_path_is_used() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        assert_eq!(
            start_path(Some(path.clone())).unwrap(),
            fs::canonicalize(path).unwrap()
        );
    }

    #[test]
    fn provided_file_is_rejected() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let error = start_path(Some(path.clone())).unwrap_err();
        assert!(error.to_string().contains(path.to_str().unwrap()));
    }

    #[test]
    fn nonexistent_path_is_rejected_and_included_in_error() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("this-path-does-not-exist-for-wullie-tests");
        let error = start_path(Some(path.clone())).unwrap_err();
        assert!(error.to_string().contains(path.to_str().unwrap()));
    }
}
