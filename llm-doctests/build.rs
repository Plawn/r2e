//! Copies `../llm/*.md` into `OUT_DIR` with every ```rust fence rewritten to
//! ```rust,no_run plus two hidden lines — `use r2e::prelude::*;` and
//! `use llm_doctests::fixtures::<topic>::*;` — and emits `topics.rs` (one
//! `#[doc = include_str!(..)] pub mod <topic> {}` per topic) and
//! `fixtures.rs` (one `pub mod <topic>` per topic, backed by
//! `src/fixtures/<topic>.rs` when that file exists, empty otherwise) for
//! `src/lib.rs` to include. Fences already carrying an attribute
//! (`rust,ignore`, `rust,no_run`, …) are kept as written.

use std::fs;
use std::path::Path;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let llm = manifest.join("../llm");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed={}", llm.display());

    let mut slugs: Vec<String> = fs::read_dir(&llm)
        .unwrap_or_else(|e| panic!("reading {}: {e}", llm.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    slugs.sort();

    let fixtures_dir = manifest.join("src/fixtures");
    println!("cargo:rerun-if-changed={}", fixtures_dir.display());

    let mut topics = String::new();
    let mut fixtures = String::new();
    for slug in &slugs {
        let src = llm.join(format!("{slug}.md"));
        println!("cargo:rerun-if-changed={}", src.display());
        let ident = slug.replace('-', "_");
        let text = fs::read_to_string(&src).unwrap();
        fs::write(out.join(format!("{slug}.md")), rewrite(&text, &ident)).unwrap();
        topics.push_str(&format!(
            "#[doc = include_str!(concat!(env!(\"OUT_DIR\"), \"/{slug}.md\"))]\npub mod {ident} {{}}\n",
        ));
        let fixture = fixtures_dir.join(format!("{slug}.rs"));
        if fixture.exists() {
            println!("cargo:rerun-if-changed={}", fixture.display());
            fixtures.push_str(&format!(
                "#[path = {:?}]\npub mod {ident};\n",
                fixture.canonicalize().unwrap().display().to_string()
            ));
        } else {
            fixtures.push_str(&format!("pub mod {ident} {{}}\n"));
        }
    }
    fs::write(out.join("topics.rs"), topics).unwrap();
    fs::write(out.join("fixtures.rs"), fixtures).unwrap();
}

/// Rewrite plain ```rust fences to compile-only doctests with the prelude
/// and the topic's fixtures in scope. A block whose first line is an inner
/// attribute keeps that line first (a `use` before `#![...]` is a syntax
/// error).
fn rewrite(text: &str, ident: &str) -> String {
    let mut out = String::with_capacity(text.len() + 1024);
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "```rust" {
            // A fence inside a list item is indented; keep that indent so the
            // markdown parser still sees one fenced block.
            let indent = &line[..line.len() - line.trim_start().len()];
            out.push_str(&format!("{indent}```rust,no_run\n"));
            if lines.peek().is_some_and(|l| l.trim_start().starts_with("#![")) {
                out.push_str(lines.next().unwrap());
                out.push('\n');
            }
            out.push_str(&format!("{indent}# use r2e::prelude::*;\n"));
            out.push_str(&format!(
                "{indent}# use llm_doctests::fixtures::{ident}::*;\n"
            ));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}
