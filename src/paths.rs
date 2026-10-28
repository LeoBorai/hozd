use std::path::{Path, PathBuf};

/// Environment variable that overrides the default `~/.hozd` location.
pub const HOME_ENV: &str = "HOZD_HOME";

#[derive(Debug, thiserror::Error)]
pub enum HomeError {
    #[error(
        "cannot determine the user home directory; set {HOME_ENV} to choose where hozd keeps its data"
    )]
    NoHomeDir,
    #[error("cannot create {path}: {source}")]
    Create {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// The directory where hozd keeps its configuration, projects and database.
#[derive(Debug, Clone)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    /// Uses `$HOZD_HOME` when set, otherwise `~/.hozd`.
    pub fn resolve() -> Result<Self, HomeError> {
        match std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
            Some(dir) => Ok(Self::at(dir)),
            None => dirs::home_dir()
                .map(|home| Self::at(home.join(".hozd")))
                .ok_or(HomeError::NoHomeDir),
        }
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Creates the root and `projects/` directories when missing.
    pub fn ensure(&self) -> Result<(), HomeError> {
        for dir in [self.root.as_path(), self.projects_dir().as_path()] {
            std::fs::create_dir_all(dir).map_err(|source| HomeError::Create {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("db.sqlite3")
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.root.join("projects")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_derived_from_root() {
        let home = Home::at("/x/.hozd");
        assert_eq!(home.db_path(), Path::new("/x/.hozd/db.sqlite3"));
        assert_eq!(home.config_path(), Path::new("/x/.hozd/config.toml"));
        assert_eq!(home.projects_dir(), Path::new("/x/.hozd/projects"));
    }

    #[test]
    fn env_override_and_default() {
        // Single test mutates the env so parallel tests cannot race on it.
        // SAFETY: no other test in this crate reads HOZD_HOME.
        unsafe { std::env::set_var(HOME_ENV, "/tmp/custom-hozd") };
        assert_eq!(
            Home::resolve().unwrap().root(),
            Path::new("/tmp/custom-hozd")
        );

        unsafe { std::env::remove_var(HOME_ENV) };
        let home = Home::resolve().unwrap();
        assert!(home.root().ends_with(".hozd"));
    }

    #[test]
    fn ensure_creates_root_and_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::at(tmp.path().join("fresh"));
        home.ensure().unwrap();
        assert!(home.root().is_dir());
        assert!(home.projects_dir().is_dir());
        home.ensure().unwrap(); // idempotent
    }

    #[test]
    fn ensure_reports_path_when_not_writable() {
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        let home = Home::at(blocker.join("sub"));
        let err = home.ensure().unwrap_err();
        assert!(err.to_string().contains("file"), "{err}");
    }
}
