use serde::{Deserialize, Serialize};

use super::common::{
    Auth, Extensions, Id, Script, SecurityScheme, Server, Source, Variable, new_id,
};
use super::rest::RestRequest;

/// The kind of API a request talks to. One variant per implemented kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ApiKind {
    Rest,
}

/// The kind-specific part of a request.
///
/// Add a kind by adding one variant here and to [`ApiKind`], plus a module with its
/// types. See the `model` module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ApiRequest {
    Rest(RestRequest),
}

impl ApiRequest {
    pub fn kind(&self) -> ApiKind {
        match self {
            Self::Rest(_) => ApiKind::Rest,
        }
    }

    pub fn as_rest(&self) -> Option<&RestRequest> {
        match self {
            Self::Rest(r) => Some(r),
        }
    }
}

impl Default for ApiRequest {
    fn default() -> Self {
        Self::Rest(RestRequest::default())
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    pub id: Id,
    pub name: String,
    pub description: Option<String>,
    pub api: ApiRequest,
    pub extensions: Extensions,
}

impl Request {
    pub fn rest(name: impl Into<String>, rest: RestRequest) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            description: None,
            api: ApiRequest::Rest(rest),
            extensions: Extensions::new(),
        }
    }

    pub fn kind(&self) -> ApiKind {
        self.api.kind()
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Directory {
    pub id: Id,
    pub name: String,
    pub description: Option<String>,
    /// `None` inherits from the parent.
    pub auth: Option<Auth>,
    pub variables: Vec<Variable>,
    pub scripts: Vec<Script>,
    pub directories: Vec<Directory>,
    pub requests: Vec<Request>,
    pub extensions: Extensions,
}

impl Directory {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            ..Self::default()
        }
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a Request>) {
        out.extend(self.requests.iter());
        for d in &self.directories {
            d.collect(out);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub description: Option<String>,
    /// Version of the described API or collection.
    pub version: Option<String>,
    pub servers: Vec<Server>,
    pub variables: Vec<Variable>,
    /// `None` means no project-level authentication is set.
    pub auth: Option<Auth>,
    /// Named schemes that [`Auth::Reference`] points at.
    pub security_schemes: Vec<SecurityScheme>,
    pub scripts: Vec<Script>,
    /// Requests outside any directory.
    pub requests: Vec<Request>,
    pub directories: Vec<Directory>,
    pub source: Source,
    pub extensions: Extensions,
}

fn find_dir<'a>(dirs: &'a mut [Directory], id: &str) -> Option<&'a mut Directory> {
    for d in dirs {
        if d.id == id {
            return Some(d);
        }
        if let Some(found) = find_dir(&mut d.directories, id) {
            return Some(found);
        }
    }
    None
}

fn find_request<'a>(dirs: &'a mut [Directory], id: &str) -> Option<&'a mut Request> {
    for d in dirs {
        if let Some(r) = d.requests.iter_mut().find(|r| r.id == id) {
            return Some(r);
        }
        if let Some(found) = find_request(&mut d.directories, id) {
            return Some(found);
        }
    }
    None
}

impl Project {
    /// The directory with `id`, at any depth.
    pub fn directory_mut(&mut self, id: &str) -> Option<&mut Directory> {
        find_dir(&mut self.directories, id)
    }

    /// The request with `id`, at any depth.
    pub fn request_mut(&mut self, id: &str) -> Option<&mut Request> {
        if let Some(r) = self.requests.iter_mut().find(|r| r.id == id) {
            return Some(r);
        }
        find_request(&mut self.directories, id)
    }

    /// Adds `dir` inside the directory `parent`, or at the project root when `parent` is
    /// `None` or unknown.
    pub fn add_directory(&mut self, parent: Option<&str>, dir: Directory) {
        match parent.and_then(|id| find_dir(&mut self.directories, id)) {
            Some(p) => p.directories.push(dir),
            None => self.directories.push(dir),
        }
    }

    /// Adds `request` inside the directory `parent`, or at the project root when `parent`
    /// is `None` or unknown.
    pub fn add_request(&mut self, parent: Option<&str>, request: Request) {
        match parent.and_then(|id| find_dir(&mut self.directories, id)) {
            Some(p) => p.requests.push(request),
            None => self.requests.push(request),
        }
    }

    /// Sets the name and description of the project, directory or request `id`; `false`
    /// when this project holds none.
    pub fn set_meta(&mut self, id: &str, name: &str, description: Option<String>) -> bool {
        if self.id == id {
            self.name = name.to_owned();
            self.description = description;
        } else if let Some(d) = self.directory_mut(id) {
            d.name = name.to_owned();
            d.description = description;
        } else if let Some(r) = self.request_mut(id) {
            r.name = name.to_owned();
            r.description = description;
        } else {
            return false;
        }
        true
    }

    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            ..Self::default()
        }
    }

    /// Every request, root level first, then each directory depth-first.
    pub fn all_requests(&self) -> Vec<&Request> {
        let mut out: Vec<&Request> = self.requests.iter().collect();
        for d in &self.directories {
            d.collect(&mut out);
        }
        out
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}
