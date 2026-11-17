// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::env;
use std::fs;
use std::io::{self, BufWriter, ErrorKind, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Parser;
use wullie::registry::CratesIo;
use wullie::update::ProjectUpdates;
use wullie::{find_dependencies, find_projects, find_updates};

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

    let project_findings = find_projects(&root);
    let dependency_findings = find_dependencies(&project_findings.projects);
    let update_findings = find_updates(&dependency_findings.projects, &CratesIo::new());

    for error in &project_findings.errors {
        eprintln!("Warning: {error:#}");
    }
    for error in &dependency_findings.errors {
        eprintln!("Warning: {error:#}");
    }
    for error in &update_findings.errors {
        eprintln!("Warning: {error:#}");
    }

    let output = write_updates(io::stdout().lock(), &update_findings.projects);
    match &output {
        Err(error) if error.kind() != ErrorKind::BrokenPipe => {
            eprintln!("Error: cannot write output: {error}");
        }
        _ => {}
    }

    let had_warnings = !project_findings.errors.is_empty()
        || !dependency_findings.errors.is_empty()
        || !update_findings.errors.is_empty();
    ExitCode::from(exit_status(had_warnings, &output))
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

fn write_updates(output: impl Write, projects: &[ProjectUpdates]) -> io::Result<()> {
    let mut output = BufWriter::new(output);
    for project in projects {
        for update in &project.updates {
            writeln!(
                output,
                "{}\t{}\t{}\t{}\t{}\t{}",
                project.project.ecosystem(),
                project.project.dir().display(),
                update.dependency.name,
                update.dependency.requirement,
                update.lowest,
                update.latest
            )?;
        }
    }
    output.flush()
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
    use super::{Cli, exit_status, start_path, write_updates};
    use clap::CommandFactory;
    use std::io::{self, ErrorKind, Write};
    use std::{env, fs, path::PathBuf};
    use wullie::dependency::Dependency;
    use wullie::project::{Ecosystem, Project};
    use wullie::update::{ProjectUpdates, Update};

    struct ClosedPipe;

    impl Write for ClosedPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(ErrorKind::BrokenPipe.into())
        }
    }

    fn projects() -> Vec<ProjectUpdates> {
        vec![ProjectUpdates {
            project: Project::new(Ecosystem::Rust, PathBuf::from("/a/Cargo.toml")).unwrap(),
            updates: vec![
                Update {
                    dependency: Dependency {
                        name: "bar".to_owned(),
                        requirement: "1".to_owned(),
                    },
                    lowest: "1.0.0".to_owned(),
                    latest: "1.2.0".to_owned(),
                },
                Update {
                    dependency: Dependency {
                        name: "libc".to_owned(),
                        requirement: "0.2".to_owned(),
                    },
                    lowest: "0.2.0".to_owned(),
                    latest: "0.2.3".to_owned(),
                },
            ],
        }]
    }

    #[test]
    fn writes_six_tab_separated_columns_per_outdated_dependency() {
        let mut output = Vec::new();
        write_updates(&mut output, &projects()).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "rust\t/a\tbar\t1\t1.0.0\t1.2.0\nrust\t/a\tlibc\t0.2\t0.2.0\t0.2.3\n"
        );
    }

    #[test]
    fn closed_output_returns_broken_pipe_instead_of_panicking() {
        let error = write_updates(ClosedPipe, &projects()).unwrap_err();
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
