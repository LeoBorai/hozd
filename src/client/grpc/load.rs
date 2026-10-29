//! Schemas from local files: compiled descriptor sets and `.proto` sources.

use std::path::{Path, PathBuf};

use miette::Diagnostic;
use prost_reflect::DescriptorPool;

use super::{GrpcError, Schema};

/// Extensions of compiled `FileDescriptorSet` files.
const DESCRIPTOR_EXTENSIONS: [&str; 3] = ["bin", "pb", "desc"];

/// Loads the schema in `path`, chosen by extension: a `.bin`, `.pb` or `.desc`
/// `FileDescriptorSet`, or a `.proto` source compiled in-process.
///
/// `proto_include` lists extra roots for `.proto` imports, searched after the directory of
/// the file itself. Blocking: call it from `spawn_blocking`.
pub fn load_schema_file(path: &Path, proto_include: &[PathBuf]) -> Result<Schema, GrpcError> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        e if DESCRIPTOR_EXTENSIONS.contains(&e) => load_descriptor_set(path),
        "proto" => compile_proto(path, proto_include),
        _ => Err(GrpcError::Schema(format!(
            "unsupported schema file `{}`: expected .proto, .bin, .pb or .desc",
            path.display()
        ))),
    }
}

fn load_descriptor_set(path: &Path) -> Result<Schema, GrpcError> {
    let bytes = std::fs::read(path)
        .map_err(|e| GrpcError::Schema(format!("cannot read {}: {e}", path.display())))?;
    let pool = DescriptorPool::decode(bytes.as_slice()).map_err(|e| {
        GrpcError::Schema(format!(
            "{} is not a valid FileDescriptorSet (build it with `protoc --include_imports`): {e}",
            path.display()
        ))
    })?;
    Ok(Schema::from_pool(pool))
}

fn compile_proto(path: &Path, proto_include: &[PathBuf]) -> Result<Schema, GrpcError> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| GrpcError::Schema(format!("`{}` is not a file", path.display())))?;

    let includes = std::iter::once(dir).chain(proto_include.iter().map(PathBuf::as_path));
    let mut compiler = protox::Compiler::new(includes).map_err(describe)?;
    compiler.include_imports(true);
    compiler.open_file(name).map_err(describe)?;
    Ok(Schema::from_pool(compiler.descriptor_pool()))
}

/// `file:line:col: message` when the compiler points at a place in a file.
fn describe(err: protox::Error) -> GrpcError {
    let message = err.to_string();
    let place = err
        .labels()
        .and_then(|mut labels| labels.next())
        .zip(err.source_code())
        .and_then(|(label, source)| {
            let contents = source.read_span(label.inner(), 0, 0).ok()?;
            Some(format!(
                "{}:{}:{}",
                contents.name().unwrap_or_default(),
                contents.line() + 1,
                contents.column() + 1
            ))
        });
    GrpcError::Schema(match (place, err.file()) {
        (Some(place), _) => format!("{place}: {message}"),
        (None, Some(file)) => format!("{file}: {message}"),
        (None, None) => message,
    })
}
