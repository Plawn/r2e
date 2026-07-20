//! Build-script helper for R2E gRPC services.
//!
//! Compiles every `.proto` file found under a proto directory (default:
//! `proto/` next to `Cargo.toml`) and emits a single aggregator file,
//! `r2e_protos.rs`, into `OUT_DIR`. The aggregator nests one Rust module per
//! protobuf package (dotted packages become nested modules) and exposes the
//! combined encoded [`FileDescriptorSet`](prost_types::FileDescriptorSet) as
//! `FILE_DESCRIPTOR_SET`, ready for gRPC server reflection.
//!
//! The whole build script is one line:
//!
//! ```no_run
//! // build.rs
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     r2e_grpc_build::compile()
//! }
//! ```
//!
//! And the application includes the generated modules with the companion
//! macro from `r2e-grpc`:
//!
//! ```ignore
//! pub mod proto {
//!     r2e::r2e_grpc::include_protos!();
//! }
//!
//! use proto::greeter::{HelloReply, HelloRequest};
//!
//! #[grpc_routes(proto::greeter::greeter_server::Greeter,
//!               descriptor = proto::FILE_DESCRIPTOR_SET)]
//! impl GreeterService { /* … */ }
//! ```
//!
//! Dropping a new `.proto` under `proto/` is all it takes — the directory is
//! registered with `cargo:rerun-if-changed`, so the next build picks it up
//! without touching `build.rs`.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Re-exported for fully custom setups that outgrow [`ProtoCompiler`].
pub use tonic_prost_build;

/// Compile `proto/**/*.proto` with the default settings.
///
/// Shorthand for `ProtoCompiler::new().compile()`.
pub fn compile() -> Result<(), Box<dyn Error>> {
    ProtoCompiler::new().compile()
}

type ConfigureFn = Box<dyn FnOnce(tonic_prost_build::Builder) -> tonic_prost_build::Builder>;

/// Configurable proto compilation for R2E build scripts.
///
/// ```no_run
/// // build.rs
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     r2e_grpc_build::ProtoCompiler::new()
///         .proto_dir("api/proto")
///         .configure(|b| b.type_attribute(".", "#[derive(serde::Serialize)]"))
///         .compile()
/// }
/// ```
pub struct ProtoCompiler {
    proto_dir: PathBuf,
    configure: Option<ConfigureFn>,
}

impl Default for ProtoCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtoCompiler {
    pub fn new() -> Self {
        Self {
            proto_dir: PathBuf::from("proto"),
            configure: None,
        }
    }

    /// Override the proto source directory (default: `proto/`, resolved
    /// relative to `CARGO_MANIFEST_DIR`).
    pub fn proto_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.proto_dir = dir.into();
        self
    }

    /// Escape hatch: tweak the underlying [`tonic_prost_build::Builder`]
    /// (type attributes, well-known types, …) before compilation. The
    /// descriptor-set path is already configured and must not be overridden.
    pub fn configure(
        mut self,
        f: impl FnOnce(tonic_prost_build::Builder) -> tonic_prost_build::Builder + 'static,
    ) -> Self {
        self.configure = Some(Box::new(f));
        self
    }

    /// Compile all protos and write the `r2e_protos.rs` aggregator into
    /// `OUT_DIR`.
    ///
    /// An empty (or missing) proto directory is not an error: a stub
    /// aggregator is generated (with an empty `FILE_DESCRIPTOR_SET`) and a
    /// `cargo:warning` is emitted, so a freshly scaffolded project builds
    /// before its first `.proto` lands.
    pub fn compile(self) -> Result<(), Box<dyn Error>> {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
        let proto_dir = if self.proto_dir.is_absolute() {
            self.proto_dir.clone()
        } else {
            manifest_dir.join(&self.proto_dir)
        };
        let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);

        // Covers file edits, additions, and removals under the directory.
        println!("cargo:rerun-if-changed={}", proto_dir.display());

        let protos = collect_protos(&proto_dir)?;
        let aggregator_path = out_dir.join("r2e_protos.rs");

        if protos.is_empty() {
            println!(
                "cargo:warning=r2e-grpc-build: no .proto files under {} — generating an empty proto module",
                proto_dir.display()
            );
            std::fs::write(
                &aggregator_path,
                render_aggregator(&PackageTree::default(), false),
            )?;
            return Ok(());
        }

        let descriptor_path = out_dir.join("r2e_descriptor.bin");
        let mut builder = tonic_prost_build::configure().file_descriptor_set_path(&descriptor_path);
        if let Some(configure) = self.configure {
            builder = configure(builder);
        }
        builder.compile_protos(&protos, std::slice::from_ref(&proto_dir))?;

        // The descriptor set is the authoritative package list (it also
        // contains imported files, e.g. well-known types, which get no
        // generated .rs of their own — the existence check filters those).
        let bytes = std::fs::read(&descriptor_path)?;
        let descriptor_set =
            <prost_types::FileDescriptorSet as prost::Message>::decode(bytes.as_slice())?;
        let mut tree = PackageTree::default();
        for file in &descriptor_set.file {
            let package = file.package();
            if out_dir.join(package_filename(package)).exists() {
                tree.insert(package);
            }
        }

        std::fs::write(&aggregator_path, render_aggregator(&tree, true))?;
        Ok(())
    }
}

/// Recursively collect `.proto` files under `dir`, sorted for determinism.
/// A missing directory yields an empty list.
fn collect_protos(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut protos = Vec::new();
    if !dir.exists() {
        return Ok(protos);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "proto") {
                protos.push(path);
            }
        }
    }
    protos.sort();
    Ok(protos)
}

/// The `.rs` file prost-build emits for a package (`""` → `_.rs`).
fn package_filename(package: &str) -> String {
    if package.is_empty() {
        "_.rs".to_string()
    } else {
        format!("{package}.rs")
    }
}

/// Module tree built from dotted protobuf package names.
///
/// `#[doc(hidden)]` — public only so the aggregator rendering is testable.
#[doc(hidden)]
#[derive(Default)]
pub struct PackageTree {
    /// A file was generated for this exact package (`_.rs` at the root).
    has_file: bool,
    children: BTreeMap<String, PackageTree>,
}

impl PackageTree {
    pub fn insert(&mut self, package: &str) {
        if package.is_empty() {
            self.has_file = true;
            return;
        }
        let mut node = self;
        for segment in package.split('.') {
            node = node.children.entry(segment.to_string()).or_default();
        }
        node.has_file = true;
    }
}

/// Render the `r2e_protos.rs` aggregator.
///
/// Relative `include!`/`include_bytes!` paths resolve against the directory
/// of the file containing the invocation — the aggregator lives in `OUT_DIR`,
/// next to the generated per-package `.rs` files and the descriptor set.
///
/// `#[doc(hidden)]` — public only for tests; not part of the API.
#[doc(hidden)]
pub fn render_aggregator(tree: &PackageTree, has_descriptor: bool) -> String {
    let mut out = String::from(
        "// @generated by r2e-grpc-build — do not edit.\n\
         //\n\
         // One module per protobuf package; dotted packages are nested so the\n\
         // `super::…` cross-package paths in prost-generated code resolve.\n",
    );
    if has_descriptor {
        out.push_str(
            "pub const FILE_DESCRIPTOR_SET: &[u8] = include_bytes!(\"r2e_descriptor.bin\");\n",
        );
    } else {
        out.push_str("pub const FILE_DESCRIPTOR_SET: &[u8] = &[];\n");
    }
    render_node(tree, "", &mut out, 0);
    out
}

fn render_node(node: &PackageTree, package_path: &str, out: &mut String, depth: usize) {
    let indent = "    ".repeat(depth);
    if node.has_file {
        let _ = writeln!(
            out,
            "{indent}include!(\"{}\");",
            package_filename(package_path)
        );
    }
    for (segment, child) in &node.children {
        let child_path = if package_path.is_empty() {
            segment.clone()
        } else {
            format!("{package_path}.{segment}")
        };
        let _ = writeln!(out, "{indent}pub mod {} {{", escape_ident(segment));
        render_node(child, &child_path, out, depth + 1);
        let _ = writeln!(out, "{indent}}}");
    }
}

/// Escape package segments that collide with Rust keywords, mirroring what
/// prost-build does for module names: raw identifiers where possible, a
/// trailing underscore for the keywords that cannot be raw (`self`, …).
fn escape_ident(segment: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "as", "async", "await", "become", "box", "break", "const", "continue", "do", "dyn", "else",
        "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let", "loop",
        "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
        "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use",
        "virtual", "where", "while", "yield",
    ];
    if matches!(segment, "self" | "Self" | "super" | "crate") {
        format!("{segment}_")
    } else if KEYWORDS.contains(&segment) {
        format!("r#{segment}")
    } else {
        segment.to_string()
    }
}
