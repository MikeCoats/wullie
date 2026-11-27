// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::env;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process;
use std::time::Duration;

use etcetera::{AppStrategy, AppStrategyArgs};
use strum::IntoEnumIterator;

use crate::project::Ecosystem;

/// A cache entry could not be written.
#[derive(Debug, thiserror::Error)]
#[error("cannot write lookup cache entry {}", path.display())]
pub(crate) struct CacheError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

impl CacheError {
    fn new(path: PathBuf, source: io::Error) -> Self {
        Self { path, source }
    }
}

/// How long a cached registry lookup is used before it is fetched again.
pub(crate) const MAX_AGE: Duration = Duration::from_hours(4);

/// An on-disk cache of raw registry index bodies.
///
/// Entries are fresh for a fixed `max_age` after they were written, judged by
/// file modification time.
/// The cache is best effort: a missing or unreadable entry is a miss, and
/// callers report failed writes without failing the registry lookup.
#[derive(Debug, Clone)]
pub(crate) struct Cache {
    dir: PathBuf,
    max_age: Duration,
}

impl Cache {
    /// Creates a cache rooted at `dir`, whose entries stay fresh for `max_age`.
    ///
    /// A zero `max_age` treats every entry as stale, so lookups are fetched
    /// again and the cache is rewritten.
    pub(crate) fn new(dir: PathBuf, max_age: Duration) -> Self {
        Self { dir, max_age }
    }

    /// The user's cache directory for Wullie, in each platform's usual place:
    /// `$XDG_CACHE_HOME/wullie` (by default `~/.cache/wullie`) on Linux,
    /// `~/Library/Caches/xyz.mikecoats.wullie` on macOS, and
    /// `%LOCALAPPDATA%\MikeCoats\wullie\cache` on Windows.
    ///
    /// On macOS, a `$XDG_CACHE_HOME` that is set still wins, so anyone who
    /// already keeps their caches there finds Wullie's alongside them.
    pub(crate) fn user_dir() -> Option<PathBuf> {
        let native = || {
            Some(
                etcetera::app_strategy::choose_native_strategy(app_args())
                    .ok()?
                    .cache_dir(),
            )
        };
        if cfg!(target_os = "macos") {
            xdg_or(env::var_os("XDG_CACHE_HOME"), native)
        } else {
            native()
        }
    }

    /// Returns the entry at `key` if it was written less than `max_age` ago.
    pub(crate) fn get(&self, key: &Path) -> Option<String> {
        let path = self.entry_path(key);
        let modified = fs::metadata(&path).and_then(|m| m.modified()).ok()?;
        // A modification time in the future fails `elapsed`, so it counts as stale.
        let age = modified.elapsed().ok()?;
        if age >= self.max_age {
            return None;
        }
        fs::read_to_string(path).ok()
    }

    /// Writes the entry at `key` through a temporary file, so readers never
    /// see a partial entry.
    pub(crate) fn put(&self, key: &Path, body: &str) -> Result<(), CacheError> {
        let path = self.entry_path(key);
        let parent = path.parent().unwrap_or(&self.dir);
        fs::create_dir_all(parent).map_err(|source| CacheError::new(path.clone(), source))?;
        let mut temporary = path.clone().into_os_string();
        temporary.push(format!(".{}.tmp", process::id()));
        let temporary = PathBuf::from(temporary);
        if let Err(source) = fs::write(&temporary, body) {
            let _ = fs::remove_file(&temporary);
            return Err(CacheError::new(path, source));
        }
        fs::rename(&temporary, &path).map_err(|source| {
            let _ = fs::remove_file(&temporary);
            CacheError::new(path, source)
        })
    }

    /// Deletes entries written at least `older_than` ago, and any directories
    /// that leaves empty.
    ///
    /// Every package looked up adds a file, so without pruning the cache only
    /// ever grows.
    /// `older_than` is separate from `max_age` so that `--refresh`, whose
    /// `max_age` is zero, doesn't throw away fresh entries for other trees.
    ///
    /// Only the ecosystem directories Wullie writes are pruned, so a cache
    /// rooted somewhere shared never deletes anything else there.
    /// Symbolic links are left alone and not followed.
    /// Pruning is best effort like the rest of the cache: anything that can't
    /// be read or deleted is skipped.
    pub(crate) fn prune(&self, older_than: Duration) {
        for ecosystem in Ecosystem::iter() {
            prune_dir(
                &self.entry_path(Path::new(&ecosystem.to_string())),
                older_than,
            );
        }
    }

    /// The file holding the entry at `key`.
    ///
    /// Keys come from package names, but a name can't be used as a file name
    /// as it is. npm names are/were/sometimes case-sensitive and on
    /// case-insensitive filesystems they would share a file, so a lookup of
    /// one could be answered with the other's releases.
    ///
    /// Each part of the key keeps only lowercase ASCII letters, digits,
    /// `-`, `_` and `@`, and every other byte is written as `%` and two
    /// uppercase hex digits, e.g. `JSONStream` as `%4A%53%4F%4E%53tream`.
    fn entry_path(&self, key: &Path) -> PathBuf {
        let mut path = self.dir.clone();
        for component in key.components() {
            if let Component::Normal(part) = component {
                path.push(escape(part.as_encoded_bytes()));
            }
        }
        path
    }

    #[cfg(test)]
    pub(crate) fn age_entry(&self, key: &Path, by: Duration) {
        let file = fs::File::options()
            .write(true)
            .open(self.entry_path(key))
            .unwrap();
        file.set_modified(std::time::SystemTime::now() - by)
            .unwrap();
    }
}

/// Prunes the files and directories inside `dir`; see [`Cache::prune`].
fn prune_dir(dir: &Path, older_than: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // `file_type` doesn't follow symbolic links, so a link is neither.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            prune_dir(&path, older_than);
            // This fails, as it should, unless the directory is now empty.
            let _ = fs::remove_dir(&path);
        } else if file_type.is_file() && is_older(&entry, older_than) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Whether `entry` was modified at least `older_than` ago.
/// A modification time in the future counts as old, as it does in `get`.
fn is_older(entry: &fs::DirEntry, older_than: Duration) -> bool {
    entry
        .metadata()
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| modified.elapsed().ok().is_none_or(|age| age >= older_than))
}

/// Escapes one part of a cache key for use as a file name; see
/// [`Cache::entry_path`].
fn escape(part: &[u8]) -> String {
    let mut escaped = String::with_capacity(part.len());
    for &byte in part {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_@".contains(&byte) {
            escaped.push(char::from(byte));
        } else {
            let _ = write!(escaped, "%{byte:02X}");
        }
    }
    escaped
}

/// Who and what Wullie is, for finding its cache directory.
/// The macOS bundle ID made from these is `xyz.mikecoats.wullie`, after the
/// website at wullie.mikecoats.xyz.
fn app_args() -> AppStrategyArgs {
    AppStrategyArgs {
        top_level_domain: "xyz".to_owned(),
        author: "MikeCoats".to_owned(),
        app_name: "wullie".to_owned(),
    }
}

/// `$XDG_CACHE_HOME/wullie` when `xdg_cache_home` is set, or else the
/// directory `native` gives.
/// As the XDG spec says, a relative or empty `$XDG_CACHE_HOME` is ignored.
fn xdg_or(
    xdg_cache_home: Option<OsString>,
    native: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    match xdg_cache_home.map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => Some(dir.join("wullie")),
        _ => native(),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    use tempfile::TempDir;

    use super::{Cache, MAX_AGE, app_args, escape, xdg_or};

    #[test]
    fn macos_bundle_id_matches_the_website() {
        assert_eq!(app_args().bundle_id(), "xyz.mikecoats.wullie");
    }

    #[test]
    fn an_absolute_xdg_cache_home_wins() {
        let xdg = std::env::temp_dir();
        let dir = xdg_or(Some(xdg.clone().into()), || panic!("native dir used"));
        assert_eq!(dir, Some(xdg.join("wullie")));
    }

    #[test]
    fn an_unset_relative_or_empty_xdg_cache_home_is_ignored() {
        let native = Path::new("native").to_path_buf();
        for xdg in [None, Some("relative/cache".into()), Some("".into())] {
            assert_eq!(xdg_or(xdg, || Some(native.clone())), Some(native.clone()));
        }
    }

    #[test]
    fn escape_keeps_lowercase_names_readable() {
        assert_eq!(escape(b"serde"), "serde");
        assert_eq!(escape(b"foo-bar_2"), "foo-bar_2");
        assert_eq!(escape(b"@types"), "@types");
    }

    #[test]
    fn escape_writes_uppercase_dots_and_tildes_as_hex() {
        assert_eq!(escape(b"JSONStream"), "%4A%53%4F%4E%53tream");
        assert_eq!(escape(b"foo."), "foo%2E");
        assert_eq!(escape(b"a~b"), "a%7Eb");
        assert_eq!(escape(b"100%"), "100%25");
    }

    #[test]
    fn keys_differing_only_in_case_or_a_trailing_dot_use_different_files() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        let keys = [
            "javascript/JSONStream",
            "javascript/jsonstream",
            "javascript/foo",
            "javascript/foo.",
        ];

        for key in keys {
            cache.put(Path::new(key), key).unwrap();
        }

        for key in keys {
            assert_eq!(cache.get(Path::new(key)).as_deref(), Some(key));
        }
        // Compare as a case-insensitive filesystem would.
        let mut names: Vec<_> = fs::read_dir(temp.path().join("javascript"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_ascii_lowercase())
            .collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), keys.len());
    }

    #[test]
    fn scoped_keys_keep_their_scope_directory() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);

        cache
            .put(Path::new("javascript/@Scope/Pkg"), "body")
            .unwrap();

        assert_eq!(
            fs::read_to_string(temp.path().join("javascript/@%53cope/%50kg")).unwrap(),
            "body"
        );
    }

    #[test]
    fn prune_deletes_old_entries_and_empty_directories() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        let old = Path::new("rust/se/rd/serde");
        let fresh = Path::new("rust/se/rd/serde_json");
        let alone = Path::new("javascript/@types/node");
        for key in [old, fresh, alone] {
            cache.put(key, "body").unwrap();
        }
        cache.age_entry(old, MAX_AGE + Duration::from_secs(60));
        cache.age_entry(alone, MAX_AGE + Duration::from_secs(60));

        cache.prune(MAX_AGE);

        assert!(!temp.path().join("rust/se/rd/serde").exists());
        assert_eq!(cache.get(fresh).as_deref(), Some("body"));
        assert!(!temp.path().join("javascript/@types").exists());
        assert!(temp.path().join("javascript").is_dir());
    }

    #[test]
    fn prune_keeps_fresh_entries_even_with_a_zero_max_age() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), Duration::ZERO);
        let key = Path::new("python/requests");
        cache.put(key, "body").unwrap();

        cache.prune(MAX_AGE);

        assert!(temp.path().join(key).exists());
    }

    #[test]
    fn prune_deletes_old_temporary_files() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        let leftover = temp.path().join("python/requests.1234.tmp");
        fs::create_dir_all(leftover.parent().unwrap()).unwrap();
        fs::write(&leftover, "partial").unwrap();
        fs::File::options()
            .write(true)
            .open(&leftover)
            .unwrap()
            .set_modified(SystemTime::now() - MAX_AGE)
            .unwrap();

        cache.prune(MAX_AGE);

        assert!(!leftover.exists());
    }

    #[test]
    fn prune_only_touches_ecosystem_directories() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        let other = temp.path().join("someone-elses/file");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(&other, "keep me").unwrap();
        fs::File::options()
            .write(true)
            .open(&other)
            .unwrap()
            .set_modified(SystemTime::now() - MAX_AGE * 10)
            .unwrap();

        cache.prune(Duration::ZERO);

        assert_eq!(fs::read_to_string(other).unwrap(), "keep me");
    }

    #[cfg(unix)]
    #[test]
    fn prune_does_not_follow_symbolic_links() {
        let temp = TempDir::new().unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("file"), "keep me").unwrap();
        let cache = Cache::new(temp.path().join("cache"), MAX_AGE);
        let rust = temp.path().join("cache/rust");
        fs::create_dir_all(&rust).unwrap();
        std::os::unix::fs::symlink(&outside, rust.join("link")).unwrap();
        std::os::unix::fs::symlink(outside.join("file"), rust.join("file-link")).unwrap();

        cache.prune(Duration::ZERO);

        assert_eq!(fs::read_to_string(outside.join("file")).unwrap(), "keep me");
        assert!(rust.join("link").symlink_metadata().is_ok());
        assert!(rust.join("file-link").symlink_metadata().is_ok());
    }

    #[test]
    fn returns_a_fresh_entry() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        let key = Path::new("rust/se/rd/serde");

        cache.put(key, "body").unwrap();

        assert_eq!(cache.get(key).as_deref(), Some("body"));
    }

    #[test]
    fn misses_a_missing_entry() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);

        assert_eq!(cache.get(Path::new("rust/1/a")), None);
    }

    #[test]
    fn misses_an_entry_older_than_the_max_age() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        let key = Path::new("rust/1/a");
        cache.put(key, "body").unwrap();

        cache.age_entry(key, MAX_AGE + Duration::from_secs(60));

        assert_eq!(cache.get(key), None);
    }

    #[test]
    fn keeps_an_entry_just_under_the_max_age() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        let key = Path::new("rust/1/a");
        cache.put(key, "body").unwrap();

        cache.age_entry(key, MAX_AGE.saturating_sub(Duration::from_secs(60)));

        assert_eq!(cache.get(key).as_deref(), Some("body"));
    }

    #[test]
    fn zero_max_age_treats_every_entry_as_stale() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), Duration::ZERO);
        let key = Path::new("rust/1/a");

        cache.put(key, "body").unwrap();

        assert_eq!(cache.get(key), None);
        assert_eq!(fs::read_to_string(temp.path().join(key)).unwrap(), "body");
    }

    #[test]
    fn put_replaces_an_entry_and_leaves_no_temporary_file() {
        let temp = TempDir::new().unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        let key = Path::new("rust/2/ab");

        cache.put(key, "old").unwrap();
        cache.put(key, "new").unwrap();

        assert_eq!(cache.get(key).as_deref(), Some("new"));
        let names: Vec<_> = fs::read_dir(temp.path().join("rust/2"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec!["ab"]);
    }

    #[test]
    fn put_errors_include_the_cache_path_and_source() {
        let temp = TempDir::new().unwrap();
        let blocker = temp.path().join("blocker");
        fs::write(&blocker, "not a directory").unwrap();
        let cache = Cache::new(temp.path().to_path_buf(), MAX_AGE);
        let key = Path::new("blocker/entry");

        let error = cache.put(key, "body").unwrap_err();

        assert!(
            error
                .to_string()
                .contains("cannot write lookup cache entry")
        );
        assert!(
            error
                .to_string()
                // Joined part by part, so the separators match on Windows too.
                .contains(
                    &temp
                        .path()
                        .join("blocker")
                        .join("entry")
                        .display()
                        .to_string()
                )
        );
        assert!(error.source().is_some());
    }
}
