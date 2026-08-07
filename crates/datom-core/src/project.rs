//! Project scaffolding: creating a new datom-connect project on disk.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{CoreError, Result};

/// Schema version written into the manifest of newly created projects.
///
/// Bumped when the on-disk project layout changes in a breaking way.
pub const SCHEMA_VERSION: u32 = 1;

/// Manifest file name at the root of every datom-connect project.
pub const MANIFEST_FILE: &str = "datom.toml";

/// Create a new datom-connect project directory named `name` under `parent`.
///
/// The resulting `<parent>/<name>/` directory contains:
/// - `datasources/` — data source definitions
/// - `pipelines/` — pipeline definitions
/// - [`datom.toml`](MANIFEST_FILE) — a manifest recording the project name and
///   [`SCHEMA_VERSION`]
///
/// Returns the path to the created project directory.
///
/// # Errors
///
/// Returns [`CoreError::ProjectExists`] if the target directory already exists,
/// or [`CoreError::Io`] if any filesystem operation fails.
pub fn init_project(parent: impl AsRef<Path>, name: &str) -> Result<PathBuf> {
    let root = parent.as_ref().join(name);

    if root.exists() {
        return Err(CoreError::ProjectExists(root.display().to_string()));
    }

    fs::create_dir_all(root.join("datasources"))?;
    fs::create_dir_all(root.join("pipelines"))?;
    fs::write(root.join(MANIFEST_FILE), manifest_contents(name))?;

    Ok(root)
}

/// Find the root of the datom project containing `start`, by looking for a
/// [`datom.toml`](MANIFEST_FILE) manifest in `start` and each of its
/// ancestors. Returns `None` when no enclosing project exists.
pub fn find_project_root(start: impl AsRef<Path>) -> Option<PathBuf> {
    let mut dir = start.as_ref();
    loop {
        if dir.join(MANIFEST_FILE).is_file() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Render the contents of a new project's `datom.toml` manifest.
fn manifest_contents(name: &str) -> String {
    // `name` is emitted as a TOML basic string; escape the characters that
    // would otherwise break the manifest.
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    format!("[project]\nname = \"{escaped}\"\nschema_version = {SCHEMA_VERSION}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_project_layout() {
        let tmp = tempdir().unwrap();
        let root = init_project(tmp.path(), "demo").unwrap();

        assert_eq!(root, tmp.path().join("demo"));
        assert!(root.is_dir());
        assert!(root.join("datasources").is_dir());
        assert!(root.join("pipelines").is_dir());
        assert!(root.join(MANIFEST_FILE).is_file());
    }

    #[test]
    fn errors_when_directory_exists() {
        let tmp = tempdir().unwrap();

        init_project(tmp.path(), "demo").unwrap();
        let err = init_project(tmp.path(), "demo").unwrap_err();

        assert!(matches!(err, CoreError::ProjectExists(_)));
    }

    #[test]
    fn finds_project_root_from_root_and_nested_dirs() {
        let tmp = tempdir().unwrap();
        let root = init_project(tmp.path(), "demo").unwrap();
        let nested = root.join("pipelines").join("deeply").join("nested");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(find_project_root(&root), Some(root.clone()));
        assert_eq!(find_project_root(&nested), Some(root));
    }

    #[test]
    fn find_project_root_returns_none_outside_a_project() {
        let tmp = tempdir().unwrap();
        assert_eq!(find_project_root(tmp.path()), None);
    }

    #[test]
    fn manifest_records_name_and_schema_version() {
        let tmp = tempdir().unwrap();
        let root = init_project(tmp.path(), "demo").unwrap();

        let manifest = fs::read_to_string(root.join(MANIFEST_FILE)).unwrap();

        assert!(manifest.contains("[project]"));
        assert!(manifest.contains("name = \"demo\""));
        assert!(manifest.contains(&format!("schema_version = {SCHEMA_VERSION}")));
    }
}
