// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The command-line interface, shared by the `wullie` binary and the man page
//! generator in `xtask`.
//!
//! The longer help is kept as structured [`Section`]s, so `--help` and the man
//! page are rendered from the same text.
//! Prose is written as single-line paragraphs; clap wraps them to the
//! terminal width and roff wraps them to the page.

use std::fmt::Write as _;
use std::path::PathBuf;

use clap::Parser;

/// Shown by `--help` in place of the one-line description `-h` shows, and as
/// the man page's DESCRIPTION.
pub const LONG_ABOUT: &str = "\
Wullie - yer local groundskeeper

Finds out-of-date dependencies in Rust, Python and JavaScript projects and \
prints a unified diff that updates their manifests to the latest releases.

Wullie never changes files itself; apply the diff with git apply or patch.

Applying the diff does not install packages or update lock files; regenerate \
and recommit Cargo.lock, uv.lock, package-lock.json or the equivalent.";

/// The command-line arguments.
#[derive(Debug, Parser)]
#[command(
    version,
    author,
    about,
    long_about = LONG_ABOUT,
    after_long_help = after_long_help(),
    max_term_width = 100
)]
pub struct Cli {
    /// Directory to start searching from (defaults to the current directory)
    pub path: Option<PathBuf>,

    /// Ignore cached registry lookups and fetch them again
    #[arg(long)]
    pub refresh: bool,
}

/// A section of the longer help, shown after the options.
#[derive(Debug)]
pub struct Section {
    /// The heading used by `--help`.
    pub title: &'static str,
    /// The heading used by the man page, following man page conventions.
    pub man_title: &'static str,
    /// The section's content, in order.
    pub blocks: &'static [Block],
}

/// A piece of a [`Section`].
#[derive(Debug)]
pub enum Block {
    /// A paragraph of prose.
    Paragraph(&'static str),
    /// Lines shown exactly as written, such as a table.
    Preformatted(&'static [&'static str]),
    /// A term and its description, such as an exit status.
    Item(&'static str, &'static str),
    /// What an example does, then the command that does it.
    Example(&'static str, &'static str),
}

/// The sections of the longer help, in order.
pub const SECTIONS: &[Section] = &[
    Section {
        title: "Projects",
        man_title: "PROJECTS",
        blocks: &[
            Block::Paragraph(
                "PATH is searched recursively for project manifests, each checked against \
                 its ecosystem's public registry:",
            ),
            Block::Preformatted(&[
                "Cargo.toml       Rust         https://index.crates.io",
                "pyproject.toml   Python       https://pypi.org/simple",
                "package.json     JavaScript   https://registry.npmjs.org",
            ]),
            Block::Paragraph(
                "Ignore files and hidden directories are respected, and build and vendor \
                 directories such as target/ and node_modules/ are skipped. Dependencies on \
                 private registries, local paths, git repositories, URLs and workspaces are \
                 not checked.",
            ),
        ],
    },
    Section {
        title: "Cache",
        man_title: "FILES",
        blocks: &[Block::Paragraph(
            "Registry lookups are cached for four hours in the user cache directory, \
             e.g. $XDG_CACHE_HOME/wullie (~/.cache/wullie) on Linux. Use --refresh to \
             pick up a release published since the last lookup.",
        )],
    },
    Section {
        title: "Exit status",
        man_title: "EXIT STATUS",
        blocks: &[
            Block::Item("0", "Success, whether or not any updates were found."),
            Block::Item("2", "Something went wrong."),
            Block::Paragraph(
                "Warnings are printed to stderr, and generated diffs are printed to stdout.",
            ),
        ],
    },
    Section {
        title: "Examples",
        man_title: "EXAMPLES",
        blocks: &[
            Block::Example("Show available updates for the current directory", "wullie"),
            Block::Example(
                "Apply updates from the root of a git repository",
                "wullie | git apply",
            ),
            Block::Example(
                "Save a patch for every project under ~/code, using fresh lookups",
                "wullie --refresh ~/code > deps.patch",
            ),
        ],
    },
];

/// Renders [`SECTIONS`] for `--help`.
pub fn after_long_help() -> String {
    SECTIONS
        .iter()
        .map(help_section)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn help_section(section: &Section) -> String {
    let mut text = format!("{}:", section.title);
    let mut previous: Option<&Block> = None;
    for block in section.blocks {
        // Consecutive items form one list; everything else is spaced apart.
        let in_list = matches!((previous, block), (Some(Block::Item(..)), Block::Item(..)));
        text.push_str(if in_list || previous.is_none() {
            "\n"
        } else {
            "\n\n"
        });
        match block {
            Block::Paragraph(paragraph) => {
                let _ = write!(text, "  {paragraph}");
            }
            Block::Preformatted(lines) => {
                let lines: Vec<_> = lines.iter().map(|line| format!("    {line}")).collect();
                text.push_str(&lines.join("\n"));
            }
            Block::Item(term, description) => {
                let _ = write!(text, "  {term}  {description}");
            }
            Block::Example(description, command) => {
                let _ = write!(text, "  {description}:\n    {command}");
            }
        }
        previous = Some(block);
    }
    text
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::{Cli, after_long_help};

    #[test]
    fn cli_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn after_long_help_renders_each_kind_of_block() {
        let help = after_long_help();

        assert!(help.starts_with("Projects:\n  PATH is searched"));
        assert!(help.contains(
            "registry:\n\n    Cargo.toml       Rust         https://index.crates.io\n    pyproject.toml"
        ));
        assert!(help.contains("\n\nCache:\n  Registry lookups"));
        assert!(help.contains(
            "Exit status:\n  0  Success, whether or not any updates were found.\n  2  Something went wrong.\n\n  Warnings"
        ));
        assert!(help.ends_with("fresh lookups:\n    wullie --refresh ~/code > deps.patch"));
    }
}
