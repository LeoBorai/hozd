use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

/// User settings from `~/.hozd/config.toml`. Every field has a default.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    /// Directories searched for `.proto` imports after the importing file's own directory.
    /// A leading `~` is the user's home directory; relative paths are relative to the hozd
    /// home directory (see [`Config::proto_include_dirs`]).
    pub proto_include: Vec<String>,
}

impl Config {
    /// [`proto_include`](Self::proto_include) with `~` expanded and relative paths resolved
    /// against `hozd_home`.
    pub fn proto_include_dirs(&self, hozd_home: &Path) -> Vec<PathBuf> {
        self.proto_include
            .iter()
            .map(|entry| expand(entry, hozd_home))
            .collect()
    }

    /// Returns defaults when the file is absent; errors name the file when it is malformed.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }
}

fn expand(entry: &str, hozd_home: &Path) -> PathBuf {
    let path = match entry.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => {
            match dirs::home_dir() {
                Some(home) => home.join(rest.trim_start_matches(['/', '\\'])),
                None => PathBuf::from(entry),
            }
        }
        _ => PathBuf::from(entry),
    };
    if path.is_absolute() {
        path
    } else {
        hozd_home.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config::load(&tmp.path().join("config.toml")).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn empty_file_yields_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }

    #[test]
    fn malformed_file_names_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "this is = = not toml").unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(err.to_string().contains("config.toml"), "{err}");
    }

    fn load(text: &str) -> Result<Config, ConfigError> {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        Config::load(&path)
    }

    #[test]
    fn proto_include_defaults_to_nothing() {
        assert!(load("").unwrap().proto_include.is_empty());
        assert!(
            load("")
                .unwrap()
                .proto_include_dirs(Path::new("/h"))
                .is_empty()
        );
    }

    #[test]
    fn proto_include_expands_tilde_and_resolves_relative_paths_against_the_home() {
        let cfg = load("proto_include = [\"~/work/protos\", \"shared\", \"/abs\"]").unwrap();
        let dirs = cfg.proto_include_dirs(Path::new("/hozd"));
        assert_eq!(
            dirs,
            [
                dirs::home_dir().unwrap().join("work/protos"),
                PathBuf::from("/hozd/shared"),
                PathBuf::from("/abs"),
            ]
        );
    }

    #[test]
    fn tilde_alone_is_the_home_directory() {
        let cfg = load("proto_include = [\"~\"]").unwrap();
        assert_eq!(
            cfg.proto_include_dirs(Path::new("/hozd")),
            [dirs::home_dir().unwrap()]
        );
    }

    #[test]
    fn proto_include_of_the_wrong_type_names_the_file() {
        for text in ["proto_include = \"x\"", "proto_include = [1, 2]"] {
            let err = load(text).unwrap_err();
            assert!(matches!(err, ConfigError::Parse { .. }));
            assert!(err.to_string().contains("config.toml"), "{err}");
        }
    }
}
