//! `r2e docs --llm …` — the AI/agent-facing reference, embedded and
//! version-matched.
//!
//! The reference is authored at the repo root as a hand-written hub
//! (`llm.txt`: golden rules + the routing table "task → topic") and one
//! topic per file under `llm/`. Both are embedded here with `include_str!`,
//! so whatever `r2e` binary a project has installed prints the reference for
//! its own version. `--export` writes the whole set into the project
//! (default `docs/r2e/`) so coding agents can `Read` the files locally;
//! `r2e new` does that export for every scaffolded project and `r2e doctor`
//! warns when the exported copy is from another R2E version.

use std::error::Error;
use std::path::Path;

/// One topic file from `llm/<slug>.md`, front matter included.
pub struct Topic {
    /// File stem, also the argument to `r2e docs --llm <slug>`.
    pub slug: &'static str,
    /// The full file: front matter, `## Title`, `### TL;DR`, content.
    pub body: &'static str,
}

/// The hub: `llm.txt` at the repo root.
pub const HUB: &str = include_str!("../../../llm.txt");

/// R2E version this reference describes (the CLI is versioned with the
/// workspace).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Directory `--export` writes to when none is given.
pub const DEFAULT_EXPORT_DIR: &str = "docs/r2e";

macro_rules! topics {
    ($($slug:literal),* $(,)?) => {
        &[$(Topic { slug: $slug, body: include_str!(concat!("../../../llm/", $slug, ".md")) }),*]
    };
}

/// Every topic, in file-name order. `tests/llm_docs.rs` checks this list
/// against the `llm/` directory so a new topic cannot be forgotten here.
pub static TOPICS: &[Topic] = topics![
    "additional-plugins",
    "app-builder",
    "background-work",
    "builder-method-quick-reference",
    "coming-from-axum",
    "configuration",
    "core-concepts",
    "data-access",
    "dev-experience",
    "devservices",
    "di-beans",
    "error-handling",
    "events",
    "grpc",
    "guards",
    "handler-parameter-types",
    "interceptors",
    "lifecycle-hooks",
    "managed-resources",
    "mcp-server",
    "method-attribute-quick-reference",
    "modules",
    "multipart",
    "observability",
    "oidc-server",
    "openapi",
    "openfga",
    "plugins",
    "prometheus",
    "proxy-catch-all",
    "quick-start",
    "runtime-facade",
    "scheduled-tasks",
    "security",
    "sse",
    "static-files",
    "tenancy",
    "tenancy-datasources",
    "testing",
    "validation",
    "websockets",
];

/// Look a topic up by slug.
pub fn topic(slug: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|t| t.slug == slug)
}

/// A topic body without its front-matter block (`---` … `---` + the blank
/// line after it): what goes into the single-file concatenation.
pub fn without_front_matter(body: &str) -> &str {
    let Some(rest) = body.strip_prefix("---\n") else {
        return body;
    };
    match rest.find("\n---\n") {
        Some(end) => rest[end + "\n---\n".len()..].trim_start_matches('\n'),
        None => body,
    }
}

/// The value of one front-matter key (`tokens`, `features`, `requires`, …).
pub fn front_matter<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let rest = body.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    rest[..end]
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
        .map(str::trim)
}

/// The generated line that stamps an exported hub with the R2E version it
/// came from. `r2e doctor` reads it back.
pub fn stamp() -> String {
    format!("<!-- R2E v{VERSION} — exported by `r2e docs --llm --export`; regenerate after upgrading r2e. -->")
}

/// Read the version out of an exported hub's stamp line, if any.
pub fn stamped_version(hub: &str) -> Option<&str> {
    let first = hub.lines().next()?;
    let rest = first.strip_prefix("<!-- R2E v")?;
    let end = rest.find(' ')?;
    Some(&rest[..end])
}

/// The whole reference as one document: hub, then every topic in the hub's
/// routing order with front matter stripped (the same shape as the repo's
/// generated `llm-full.txt`).
pub fn full() -> String {
    let mut out = String::with_capacity(HUB.len() * 30);
    out.push_str(&stamp());
    out.push_str("\n\n");
    out.push_str(HUB);
    for t in routing_order() {
        out.push_str("\n---\n\n");
        out.push_str(without_front_matter(t.body));
    }
    out
}

/// Topics in order of first reference from the hub; anything the hub does
/// not reference (which the repo check forbids) is appended in slug order.
fn routing_order() -> Vec<&'static Topic> {
    let mut ordered: Vec<&'static Topic> = Vec::with_capacity(TOPICS.len());
    let mut rest = HUB;
    while let Some(i) = rest.find("llm/") {
        let after = &rest[i + "llm/".len()..];
        let len = after
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
            .unwrap_or(after.len());
        let slug = &after[..len];
        if after[len..].starts_with(".md") {
            if let Some(t) = topic(slug) {
                if !ordered.iter().any(|o| o.slug == t.slug) {
                    ordered.push(t);
                }
            }
        }
        rest = &after[len..];
    }
    for t in TOPICS {
        if !ordered.iter().any(|o| o.slug == t.slug) {
            ordered.push(t);
        }
    }
    ordered
}

/// Write the hub (stamped) and every topic under `dir`: `<dir>/llm.txt` and
/// `<dir>/llm/<slug>.md`. Existing files are overwritten — the export is the
/// version-matched copy, never a place to edit. Returns the number of files
/// written.
pub fn export(dir: &Path) -> Result<usize, Box<dyn Error>> {
    let topics_dir = dir.join("llm");
    std::fs::create_dir_all(&topics_dir)?;
    std::fs::write(dir.join("llm.txt"), format!("{}\n\n{}", stamp(), HUB))?;
    for t in TOPICS {
        std::fs::write(topics_dir.join(format!("{}.md", t.slug)), t.body)?;
    }
    Ok(1 + TOPICS.len())
}

/// Entry point for `r2e docs --llm`.
///
/// - no topic, no flag → the hub (index + routing table)
/// - `<topic>` → that topic file
/// - `--full` → hub + every topic, one document
/// - `--export [DIR]` → write hub + topics under DIR (default `docs/r2e`)
pub fn run(
    topic_slug: Option<&str>,
    full_doc: bool,
    export_dir: Option<&Path>,
    pretty: bool,
) -> Result<(), Box<dyn Error>> {
    // Rendering is for a human at a terminal; agents want the raw markdown.
    let emit = |text: &str| {
        if pretty {
            termimad::print_text(text);
        } else {
            print!("{text}");
        }
    };
    if let Some(dir) = export_dir {
        let n = export(dir)?;
        println!(
            "Exported the R2E v{VERSION} agent reference: {n} files under {} (hub: {}/llm.txt).",
            dir.display(),
            dir.display()
        );
        return Ok(());
    }
    if full_doc {
        emit(&full());
        return Ok(());
    }
    match topic_slug {
        None => emit(HUB),
        Some(slug) => match topic(slug) {
            Some(t) => emit(if pretty { without_front_matter(t.body) } else { t.body }),
            None => {
                let slugs: Vec<&str> = TOPICS.iter().map(|t| t.slug).collect();
                return Err(format!(
                    "unknown topic `{slug}`. Available: {}",
                    slugs.join(", ")
                )
                .into());
            }
        },
    }
    Ok(())
}
