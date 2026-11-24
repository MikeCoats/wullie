// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

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
use wullie::registry::{Cache, HttpRegistry, MAX_AGE};
use wullie::{find_dependencies, find_projects, find_updates, manifest_diffs};

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
    if let Some(error) = registry.cache_write_error() {
        warn(error);
    }
    if cache_unavailable {
        write_stderr(
            "Warning: cannot determine user cache directory; registry lookups will not be cached",
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
    let _ = writeln!(io::stderr().lock(), "{message}");
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
    use super::{chain, exit_status, start_path, write_diff};
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
