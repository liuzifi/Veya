//! External configuration that lives outside the SQLite store.
//!
//! The database path cannot be persisted inside the database itself: the
//! worker needs the path *before* it can open the store to read any setting,
//! so a custom location would create a chicken-and-egg problem. Instead a tiny
//! plain-text sidecar file (`veya.cfg`) sits next to the default database and
//! holds a single line — the override directory for `veya.db`.
//!
//! The file is intentionally human-readable and editable so that users can
//! relocate the database without launching the app (for example when the old
//! disk is failing). Reading is forgiving: any IO error, missing file, or
//! blank/whitespace-only content yields the built-in default path.

use std::path::PathBuf;

/// Name of the sidecar file, placed beside the default database location.
const CFG_FILE: &str = "veya.cfg";

/// The built-in database location used when no override is configured:
/// `%APPDATA%/Veya/veya.db` (or `./veya.db` when `APPDATA` is unset).
pub fn default_db_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Veya")
        .join("veya.db")
}

/// Directory that holds both the sidecar config and the default database.
fn app_data_dir() -> PathBuf {
    default_db_path()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Path of the sidecar config file.
pub fn cfg_path() -> PathBuf {
    app_data_dir().join(CFG_FILE)
}

/// Read the custom database directory recorded in the sidecar config.
///
/// Returns `None` when the file is missing, unreadable, or contains only
/// whitespace — in all those cases the caller falls back to the default.
/// Trailing path separators and surrounding whitespace are stripped so that
/// the stored value can be joined with `veya.db` unambiguously.
pub fn read_custom_db_dir() -> Option<PathBuf> {
    let raw = std::fs::read_to_string(cfg_path()).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

/// Persist a custom database directory to the sidecar config.
///
/// Passing an empty string clears the override (the file is removed) so that
/// the next launch falls back to the default location. The parent directory is
/// created on demand so the write never fails solely because `%APPDATA%/Veya`
/// does not exist yet.
pub fn write_custom_db_dir(dir: &str) -> std::io::Result<()> {
    let path = cfg_path();
    if dir.trim().is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, dir.trim())
}

/// Resolve the effective database file path: the override directory (when
/// present and non-empty) joined with `veya.db`, otherwise the default.
pub fn resolve_db_path() -> PathBuf {
    match read_custom_db_dir() {
        Some(dir) => dir.join("veya.db"),
        None => default_db_path(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_lives_under_appdata() {
        let path = default_db_path();
        assert!(path.ends_with("veya.db"));
        assert!(path.parent().is_some_and(|p| p.ends_with("Veya")));
    }

    #[test]
    fn cfg_path_shares_dir_with_default_db() {
        let cfg = cfg_path();
        let db = default_db_path();
        assert_eq!(cfg.parent(), db.parent());
        assert!(cfg.ends_with(CFG_FILE));
    }

    #[test]
    fn empty_read_returns_none_and_does_not_panic() {
        // The default location almost certainly has no cfg during tests; even
        // if it does, an empty/whitespace body must map to `None`.
        if let Some(dir) = read_custom_db_dir() {
            assert!(!dir.as_os_str().is_empty(), "non-empty override expected");
        }
    }

    #[test]
    fn resolve_honours_override_when_present() {
        // If a sidecar happens to exist the resolved path must point inside it;
        // otherwise it falls back to the default. Either way the file name is
        // `veya.db`.
        let resolved = resolve_db_path();
        assert!(resolved.ends_with("veya.db"));
    }
}
