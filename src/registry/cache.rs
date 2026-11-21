// SPDX-FileCopyrightText: 2026 Mike Coats <i.am@mikecoats.com>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

use etcetera::{AppStrategy, AppStrategyArgs};

/// A cache entry could not be written.
#[derive(Debug, thiserror::Error)]
#[error("cannot write lookup cache entry {}", path.display())]
pub struct CacheError {
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
pub const MAX_AGE: Duration = Duration::from_secs(4 * 60 * 60);

/// An on-disk cache of raw registry index bodies.
///
/// Entries are fresh for a fixed `max_age` after they were written, judged by
/// file modification time.
/// The cache is best effort: a missing or unreadable entry is a miss, and
/// callers report failed writes without failing the registry lookup.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
    max_age: Duration,
}

impl Cache {
    /// Creates a cache rooted at `dir`, whose entries stay fresh for `max_age`.
    ///
    /// A zero `max_age` treats every entry as stale, so lookups are fetched
    /// again and the cache is rewritten.
    pub fn new(dir: PathBuf, max_age: Duration) -> Self {
        Self { dir, max_age }
    }

    /// The user's cache directory for Wullie, e.g. `$XDG_CACHE_HOME/wullie`.
    pub fn user_dir() -> Option<PathBuf> {
        let strategy = etcetera::choose_app_strategy(AppStrategyArgs {
            top_level_domain: "com".to_owned(),
            author: "Mike Coats".to_owned(),
            app_name: "wullie".to_owned(),
        })
        .ok()?;
        Some(strategy.cache_dir())
    }

    /// Returns the entry at `key` if it was written less than `max_age` ago.
    pub(crate) fn get(&self, key: &Path) -> Option<String> {
        let path = self.dir.join(key);
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
        let path = self.dir.join(key);
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

    #[cfg(test)]
    pub(crate) fn age_entry(&self, key: &Path, by: Duration) {
        let file = fs::File::options()
            .write(true)
            .open(self.dir.join(key))
            .unwrap();
        file.set_modified(std::time::SystemTime::now() - by)
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::{Cache, MAX_AGE};

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

        cache.age_entry(key, MAX_AGE - Duration::from_secs(60));

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
                .contains(&temp.path().join(key).display().to_string())
        );
        assert!(error.source().is_some());
    }
}
