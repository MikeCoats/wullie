// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Walks a directory tree for manifests, skipping hidden, ignored and
//! dependency directories.

use std::path::{Path, PathBuf};

use ignore::{DirEntry, WalkBuilder};

/// An error encountered while walking the directory tree.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct WalkError(ignore::Error);

/// Recursively lists files under `root`, respecting ignore files and skipping
/// hidden entries and any directory whose name is in `skip`.
///
/// Walk errors are yielded rather than dropped, so callers can report them.
/// This includes problems with ignore files, such as an invalid glob, which
/// `ignore` attaches to the directory entry instead of yielding as an error.
pub fn files(
    root: &Path,
    skip: &[&'static str],
) -> impl Iterator<Item = Result<PathBuf, WalkError>> {
    let skip_dirs = skip.to_vec();
    let mut builder = WalkBuilder::new(root);

    builder
        .hidden(true)
        .require_git(false)
        // Results are re-sorted by the caller; sorting here keeps warnings in a stable order.
        .sort_by_file_name(Ord::cmp)
        .filter_entry(move |entry: &DirEntry| {
            if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_dir()) {
                return true;
            }

            !skip_dirs.iter().any(|name| entry.file_name() == *name)
        });

    builder.build().flat_map(|entry| match entry {
        Ok(entry) => {
            let error = entry.error().cloned().map(WalkError).map(Err);
            let file = entry
                .file_type()
                .is_some_and(|kind| kind.is_file())
                .then(|| Ok(entry.into_path()));
            error.into_iter().chain(file)
        }
        Err(error) => Some(Err(WalkError(error))).into_iter().chain(None),
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
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

        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn reports_invalid_ignore_files_and_continues() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("file.txt");
        fs::write(temp.path().join(".gitignore"), "{a\n").unwrap();
        fs::write(&file, "content").unwrap();

        let (found, errors): (Vec<_>, Vec<_>) = files(temp.path(), &[]).partition(Result::is_ok);

        assert_eq!(
            found.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            vec![file]
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].as_ref().unwrap_err().to_string().contains("{a"));
    }

    #[test]
    fn walk_error_preserves_display_and_source() {
        let inner = ignore::Error::from(std::io::Error::other("walk failed"));
        let source = StdError::source(&inner).map(ToString::to_string);
        let error = super::WalkError(inner);

        assert_eq!(error.to_string(), "walk failed");
        assert_eq!(StdError::source(&error).map(ToString::to_string), source);
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

        assert!(found.is_empty(), "{found:?}");
    }
}
