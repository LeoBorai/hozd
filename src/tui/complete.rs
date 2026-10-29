//! Path suggestions for the schema file input.

use std::path::{Path, PathBuf};

/// File extensions a schema can have; other files are not suggested.
const SCHEMA_EXTENSIONS: [&str; 4] = ["proto", "bin", "pb", "desc"];

/// Most suggestions kept for one input.
const LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// The whole input once accepted: the typed directory part plus the entry, with a
    /// trailing `/` for directories.
    pub text: String,
    pub dir: bool,
}

/// The directories and schema files that continue `typed`, directories first. `~/` is
/// understood; the suggestions keep the form the user typed.
pub fn complete(typed: &str) -> Vec<Suggestion> {
    let (dir_part, prefix) = match typed.rfind('/') {
        Some(i) => typed.split_at(i + 1),
        None => ("", typed),
    };
    let Ok(entries) = std::fs::read_dir(expand(dir_part)) else {
        return Vec::new();
    };
    let show_hidden = prefix.starts_with('.');
    let mut found: Vec<Suggestion> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with(prefix) || (name.starts_with('.') && !show_hidden) {
                return None;
            }
            // `is_dir` follows symlinks, so a link to a directory is one too
            if entry.path().is_dir() {
                return Some(Suggestion {
                    text: format!("{dir_part}{name}/"),
                    dir: true,
                });
            }
            let ext = Path::new(&name).extension()?.to_str()?;
            SCHEMA_EXTENSIONS.contains(&ext).then(|| Suggestion {
                text: format!("{dir_part}{name}"),
                dir: false,
            })
        })
        .collect();
    found.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.text.cmp(&b.text)));
    found.truncate(LIMIT);
    found
}

fn expand(dir_part: &str) -> PathBuf {
    if dir_part.is_empty() {
        return PathBuf::from(".");
    }
    match dir_part.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(dir_part),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(found: &[Suggestion]) -> Vec<&str> {
        found.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn suggests_directories_first_then_schema_files_matching_the_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        for file in ["demo.proto", "data.bin", "notes.txt", ".hidden.proto"] {
            std::fs::write(dir.path().join(file), "").unwrap();
        }
        std::fs::create_dir(dir.path().join("deps")).unwrap();

        let all = complete(&format!("{root}/"));
        assert_eq!(
            texts(&all),
            [
                format!("{root}/deps/"),
                format!("{root}/data.bin"),
                format!("{root}/demo.proto"),
            ]
        );
        assert!(all[0].dir && !all[1].dir);

        let de = complete(&format!("{root}/de"));
        assert_eq!(
            texts(&de),
            [format!("{root}/deps/"), format!("{root}/demo.proto")]
        );
        assert_eq!(
            texts(&complete(&format!("{root}/."))),
            [format!("{root}/.hidden.proto")]
        );
        assert!(complete(&format!("{root}/zzz")).is_empty());
        assert!(complete("/definitely/not/here/x").is_empty());
    }
}
