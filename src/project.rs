mod template;

use std::path::{Path, PathBuf};

pub use template::{GrpcTemplate, HttpTemplate, Template, TemplateError, TemplateKind};

/// A project: one directory under `projects/` holding request templates.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub templates: Vec<LoadedTemplate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedTemplate {
    /// Path relative to the project directory, e.g. `users/create.toml`.
    pub path: PathBuf,
    pub template: Template,
}

/// A template file that could not be loaded.
#[derive(Debug)]
pub struct LoadIssue {
    pub path: PathBuf,
    pub error: TemplateError,
}

impl std::fmt::Display for LoadIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.error)
    }
}

#[derive(Debug, Default)]
pub struct Loaded {
    pub projects: Vec<Project>,
    pub issues: Vec<LoadIssue>,
}

/// Loads every project under `projects_dir`. A missing directory means no projects;
/// a broken template is reported in `issues` and never hides the others.
pub fn load_projects(projects_dir: &Path) -> Loaded {
    let mut loaded = Loaded::default();
    let Ok(entries) = std::fs::read_dir(projects_dir) else {
        return loaded;
    };

    let mut dirs: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .collect();
    dirs.sort_by_key(|e| e.file_name());

    for dir in dirs {
        let mut project = Project {
            name: dir.file_name().to_string_lossy().into_owned(),
            templates: Vec::new(),
        };
        collect(&dir.path(), &dir.path(), &mut project, &mut loaded.issues);
        loaded.projects.push(project);
    }
    loaded
}

fn collect(root: &Path, dir: &Path, project: &mut Project, issues: &mut Vec<LoadIssue>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();

    for path in paths {
        if path.is_dir() {
            collect(root, &path, project, issues);
        } else if path.extension().is_some_and(|ext| ext == "toml") {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            match Template::from_file(&path) {
                Ok(template) => project.templates.push(LoadedTemplate {
                    path: rel,
                    template,
                }),
                Err(error) => issues.push(LoadIssue { path, error }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    const OK: &str = "name = \"n\"\nmethod = \"GET\"\nurl = \"http://x\"\n";

    #[test]
    fn missing_dir_is_zero_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = load_projects(&tmp.path().join("nope"));
        assert!(loaded.projects.is_empty() && loaded.issues.is_empty());
    }

    #[test]
    fn nested_templates_are_listed_and_non_toml_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "api/users/create.toml", OK);
        write(tmp.path(), "api/health.toml", OK);
        write(tmp.path(), "api/README.md", "ignore me");
        let loaded = load_projects(tmp.path());

        assert_eq!(loaded.projects.len(), 1);
        let paths: Vec<_> = loaded.projects[0]
            .templates
            .iter()
            .map(|t| t.path.to_str().unwrap().to_owned())
            .collect();
        assert_eq!(paths, ["health.toml", "users/create.toml"]);
        assert!(loaded.issues.is_empty());
    }

    #[test]
    fn broken_file_is_reported_and_others_still_load() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "api/good.toml", OK);
        write(tmp.path(), "api/syntax.toml", "= nope");
        write(
            tmp.path(),
            "api/method.toml",
            "name = \"n\"\nmethod = \"TRACE\"\nurl = \"http://x\"\n",
        );
        let loaded = load_projects(tmp.path());

        assert_eq!(loaded.projects[0].templates.len(), 1);
        assert_eq!(loaded.issues.len(), 2);
        assert!(
            loaded
                .issues
                .iter()
                .any(|i| i.path.ends_with("syntax.toml"))
        );
        assert!(
            loaded
                .issues
                .iter()
                .any(|i| i.path.ends_with("method.toml"))
        );
    }
}
