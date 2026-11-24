// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Project automation for Wullie, run with `cargo xtask <TASK>`.
//!
//! Generates the files that are built from the command-line definition in
//! `wullie::cli`: the man page and shell completions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use clap_mangen::Man;
use clap_mangen::roff::{Roff, bold, roman};
use wullie::cli::{Block, Cli, SECTIONS, Section};

#[derive(Parser)]
#[command(about = "Project automation for Wullie")]
struct Xtask {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Generate the wullie(1) man page
    Man {
        /// Directory to write wullie.1 into (defaults to target/man)
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },
    /// Generate shell completions for every shell clap supports
    Completions {
        /// Directory to write the completion scripts into (defaults to target/completions)
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Xtask::parse().task {
        Task::Man { out_dir } => {
            let out_dir = out_dir.unwrap_or_else(|| workspace_root().join("target/man"));
            let path = write_man_page(&out_dir)?;
            println!("{}", path.display());
            Ok(())
        }
        Task::Completions { out_dir } => {
            let out_dir = out_dir.unwrap_or_else(|| workspace_root().join("target/completions"));
            for path in write_completions(&out_dir)? {
                println!("{}", path.display());
            }
            Ok(())
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

fn write_man_page(out_dir: &Path) -> Result<PathBuf> {
    let mut man = Man::new(Cli::command()).manual("User Commands");
    if let Some(date) = last_commit_date() {
        man = man.date(date);
    }

    let mut page = Vec::new();
    man.render_title(&mut page)?;
    man.render_name_section(&mut page)?;
    man.render_synopsis_section(&mut page)?;
    man.render_description_section(&mut page)?;
    man.render_options_section(&mut page)?;
    for section in SECTIONS {
        man_section(section).to_writer(&mut page)?;
    }
    man.render_version_section(&mut page)?;
    man.render_authors_section(&mut page)?;

    fs::create_dir_all(out_dir).with_context(|| format!("cannot create {}", out_dir.display()))?;
    let path = out_dir.join(man.get_filename());
    fs::write(&path, page).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

/// Writes a completion script for each shell, named as that shell expects,
/// e.g. `wullie.bash`, `_wullie` for zsh and `wullie.fish`.
fn write_completions(out_dir: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(out_dir).with_context(|| format!("cannot create {}", out_dir.display()))?;
    let mut cli = Cli::command();
    let name = cli.get_name().to_owned();
    Shell::value_variants()
        .iter()
        .map(|&shell| {
            clap_complete::generate_to(shell, &mut cli, &name, out_dir).with_context(|| {
                format!("cannot write {shell} completions to {}", out_dir.display())
            })
        })
        .collect()
}

/// The date of the last commit, so the page is reproducible for a given
/// commit, as `just dist` does for the tarball.
fn last_commit_date() -> Option<String> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%cs"])
        .current_dir(workspace_root())
        .output()
        .ok()?;
    let date = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (output.status.success() && !date.is_empty()).then_some(date)
}

fn man_section(section: &Section) -> Roff {
    let mut roff = Roff::new();
    roff.control("SH", [section.man_title]);
    let mut previous: Option<&Block> = None;
    for block in section.blocks {
        match block {
            Block::Paragraph(paragraph) => {
                if previous.is_some() {
                    roff.control("PP", []);
                }
                roff.text([roman(*paragraph)]);
            }
            Block::Preformatted(lines) => {
                if previous.is_some() {
                    roff.control("sp", []);
                }
                roff.control("RS", ["4"]).control("nf", []);
                for line in *lines {
                    roff.text([roman(*line)]);
                }
                roff.control("fi", []).control("RE", []);
            }
            Block::Item(term, description) => {
                roff.control("TP", [])
                    .text([bold(*term)])
                    .text([roman(*description)]);
            }
            Block::Example(description, command) => {
                if previous.is_some() {
                    roff.control("PP", []);
                }
                roff.text([roman(format!("{description}:"))])
                    .control("RS", ["4"])
                    .control("nf", [])
                    .text([bold(*command)])
                    .control("fi", [])
                    .control("RE", []);
            }
        }
        previous = Some(block);
    }
    roff
}
