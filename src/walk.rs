// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::{Path, PathBuf};

use ignore::{DirEntry, WalkBuilder};

/// Recursively lists files under `root`, respecting ignore files and skipping
/// hidden entries and any directory whose name is in `skip`.
///
/// Walk errors are yielded rather than dropped, so callers can report them.
pub fn files(
    root: &Path,
    skip: &[&'static str],
) -> impl Iterator<Item = Result<PathBuf, ignore::Error>> {
    let skip_dirs = skip.to_vec();
    let mut builder = WalkBuilder::new(root);

    builder
        .hidden(true)
        .require_git(false)
        // Results are re-sorted by the caller; sorting here keeps warnings in a stable order.
        .sort_by_file_name(|left, right| left.cmp(right))
        .filter_entry(move |entry: &DirEntry| {
            if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_dir()) {
                return true;
            }

            !skip_dirs.iter().any(|name| entry.file_name() == *name)
        });

    builder.build().filter_map(|entry| match entry {
        Ok(entry) if entry.file_type().is_some_and(|kind| kind.is_file()) => {
            Some(Ok(entry.into_path()))
        }
        Ok(_) => None,
        Err(error) => Some(Err(error)),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::files;

    #[test]
    fn finds_nested_files() {
        let temp = TempDir::new().unwrap();
        let nested = temp.path().join("one/two/file.txt");
        fs::create_dir_all(nested.parent().unwrap()).unwrap();
        fs::write(&nested, "content").unwrap();

        let found = files(temp.path(), &[])
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(found, vec![nested]);
    }

    #[test]
    fn prunes_named_directories() {
        let temp = TempDir::new().unwrap();
        let included = temp.path().join("included.txt");
        let pruned = temp.path().join("target/file.txt");
        fs::write(&included, "content").unwrap();
        fs::create_dir_all(pruned.parent().unwrap()).unwrap();
        fs::write(&pruned, "content").unwrap();

        let found = files(temp.path(), &["target"])
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(found, vec![included]);
    }

    #[test]
    fn respects_gitignore_outside_a_repository() {
        let temp = TempDir::new().unwrap();
        let ignored = temp.path().join("ignored/file.txt");
        fs::write(temp.path().join(".gitignore"), "ignored/\n").unwrap();
        fs::create_dir_all(ignored.parent().unwrap()).unwrap();
        fs::write(&ignored, "content").unwrap();

        let found = files(temp.path(), &[])
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(found.is_empty());
    }

    #[test]
    fn skips_hidden_directories() {
        let temp = TempDir::new().unwrap();
        let hidden = temp.path().join(".hidden/file.txt");
        fs::create_dir_all(hidden.parent().unwrap()).unwrap();
        fs::write(&hidden, "content").unwrap();

        let found = files(temp.path(), &[])
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(found.is_empty());
    }
}
