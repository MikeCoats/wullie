// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Parser;

/// Wullie - yer local groundskeeper
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Directory to start searching from (defaults to the current directory)
    path: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let root = match start_path(cli.path) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Error: {error:#}");
            return ExitCode::from(2);
        }
    };

    let findings = wullie::find_projects(&root);

    for project in &findings.projects {
        println!("{}\t{}", project.ecosystem(), project.dir().display());
    }

    for error in &findings.errors {
        eprintln!("Warning: {error}");
    }

    ExitCode::SUCCESS
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
    use super::{Cli, start_path};
    use clap::CommandFactory;
    use std::{env, fs, path::PathBuf};

    #[test]
    fn cli_is_valid() {
        Cli::command().debug_assert();
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
