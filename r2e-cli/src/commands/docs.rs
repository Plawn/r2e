//! `r2e docs [<module>]` — print bundled, version-matched module documentation.
//!
//! Each entry maps a clean English slug to one of the `docs/features/*.md`
//! files, embedded at compile time via `include_str!`. By default the command
//! prints the curated `## TL;DR` section of a module; `--full` prints the whole
//! document and `--pretty` renders markdown for a terminal reader.

use std::error::Error;

/// A single embedded module document.
struct DocEntry {
    /// Clean English slug used as the command argument (e.g. `events`).
    slug: &'static str,
    /// Human-readable title shown in the listing.
    title: &'static str,
    /// Owning crate(s), also accepted as aliases.
    crates: &'static [&'static str],
    /// The full markdown document, embedded at compile time.
    body: &'static str,
}

/// The bundled documentation set — one entry per `docs/features/*.md` file.
static DOCS: &[DocEntry] = &[
    DocEntry {
        slug: "configuration",
        title: "Configuration",
        crates: &["r2e-core"],
        body: include_str!("../../../docs/features/01-configuration.md"),
    },
    DocEntry {
        slug: "validation",
        title: "Validation",
        crates: &["r2e-core"],
        body: include_str!("../../../docs/features/02-validation.md"),
    },
    DocEntry {
        slug: "error-handling",
        title: "Error Handling",
        crates: &["r2e-core"],
        body: include_str!("../../../docs/features/03-error-handling.md"),
    },
    DocEntry {
        slug: "interceptors",
        title: "Interceptors",
        crates: &["r2e-macros"],
        body: include_str!("../../../docs/features/04-intercepteurs.md"),
    },
    DocEntry {
        slug: "openapi",
        title: "OpenAPI",
        crates: &["r2e-openapi"],
        body: include_str!("../../../docs/features/05-openapi.md"),
    },
    DocEntry {
        slug: "data-repository",
        title: "Pagination & Managed Transactions",
        crates: &["r2e-core"],
        body: include_str!("../../../docs/features/06-data-repository.md"),
    },
    DocEntry {
        slug: "events",
        title: "Events",
        crates: &["r2e-events"],
        body: include_str!("../../../docs/features/07-evenements.md"),
    },
    DocEntry {
        slug: "scheduling",
        title: "Scheduling",
        crates: &["r2e-scheduler"],
        body: include_str!("../../../docs/features/08-scheduling.md"),
    },
    DocEntry {
        slug: "dev-mode",
        title: "Development Mode",
        crates: &["r2e-core"],
        body: include_str!("../../../docs/features/09-dev-mode.md"),
    },
    DocEntry {
        slug: "lifecycle-hooks",
        title: "Lifecycle Hooks",
        crates: &["r2e-core"],
        body: include_str!("../../../docs/features/10-lifecycle-hooks.md"),
    },
    DocEntry {
        slug: "security",
        title: "JWT Security / Roles",
        crates: &["r2e-security"],
        body: include_str!("../../../docs/features/11-securite-jwt.md"),
    },
    DocEntry {
        slug: "testing",
        title: "Testing",
        crates: &["r2e-test"],
        body: include_str!("../../../docs/features/12-testing.md"),
    },
    DocEntry {
        slug: "lifecycle-performance",
        title: "Lifecycle, DI & Performance",
        crates: &["r2e-core", "r2e-macros"],
        body: include_str!("../../../docs/features/13-lifecycle-injection-performance.md"),
    },
    DocEntry {
        slug: "websocket",
        title: "WebSocket",
        crates: &["r2e-http"],
        body: include_str!("../../../docs/features/14-websocket.md"),
    },
    DocEntry {
        slug: "sse",
        title: "Server-Sent Events",
        crates: &["r2e-http"],
        body: include_str!("../../../docs/features/15-sse.md"),
    },
    DocEntry {
        slug: "multipart",
        title: "Multipart (File Upload)",
        crates: &["r2e-http"],
        body: include_str!("../../../docs/features/16-multipart.md"),
    },
    DocEntry {
        slug: "grpc",
        title: "gRPC",
        crates: &["r2e-grpc"],
        body: include_str!("../../../docs/features/17-grpc.md"),
    },
    DocEntry {
        slug: "quic",
        title: "QUIC / HTTP/3",
        crates: &["r2e-http"],
        body: include_str!("../../../docs/features/18-quic.md"),
    },
    DocEntry {
        slug: "sharded-serving",
        title: "Sharded Serving (SO_REUSEPORT)",
        crates: &["r2e-core"],
        body: include_str!("../../../docs/features/19-sharded-serving.md"),
    },
    DocEntry {
        slug: "proxy-catch-all",
        title: "Proxy & Catch-All Routes",
        crates: &["r2e-macros", "r2e-core"],
        body: include_str!("../../../docs/features/20-proxy-catch-all.md"),
    },
    DocEntry {
        slug: "dynamic-scheduled-tasks",
        title: "Dynamic (Config-Driven) Scheduled Tasks",
        crates: &["r2e-scheduler"],
        body: include_str!("../../../docs/features/21-dynamic-scheduled-tasks.md"),
    },
    DocEntry {
        slug: "serve-lifecycle",
        title: "Serve Lifecycle (Stop & Drain)",
        crates: &["r2e-core", "r2e-grpc"],
        body: include_str!("../../../docs/features/22-serve-lifecycle.md"),
    },
];

/// Extract the `## TL;DR` section from a document body: everything from the
/// `## TL;DR` heading up to (but excluding) the next `## ` heading. Returns the
/// trimmed section text (heading included), or `None` if there is no TL;DR.
pub fn tldr(body: &str) -> Option<&str> {
    let start = find_heading(body, "## TL;DR")?;
    let after = &body[start..];
    // Skip past the heading line itself before looking for the next `## `.
    let heading_end = after
        .find('\n')
        .map(|i| start + 1 +i)
        .unwrap_or(body.len());
    let end = match find_heading(&body[heading_end..], "## ") {
        Some(rel) => heading_end + rel,
        None => body.len(),
    };
    Some(body[start..end].trim_end())
}

/// Find the byte offset of a line that starts with `needle` (a markdown
/// heading), respecting line boundaries so it never matches mid-line.
fn find_heading(body: &str, needle: &str) -> Option<usize> {
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        if line.starts_with(needle) {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

/// Format the `slug — Title (crate[, crate])` label for a listing row.
fn label(entry: &DocEntry) -> String {
    format!(
        "{} — {} ({})",
        entry.slug,
        entry.title,
        entry.crates.join(", ")
    )
}

/// Print `text`, rendering markdown for a terminal when `pretty` is set.
fn emit(text: &str, pretty: bool) {
    if pretty {
        termimad::print_text(text);
    } else {
        println!("{text}");
    }
}

/// Print the sorted listing of every available module.
fn print_list() {
    let mut rows: Vec<&DocEntry> = DOCS.iter().collect();
    rows.sort_by_key(|e| e.slug);

    println!("Available modules:\n");
    for entry in rows {
        println!("  {}", label(entry));
    }
    println!(
        "\nRun `r2e docs <module>` to read one (add --full for the whole doc, --pretty to render)."
    );
}

/// Look up a module by exact slug.
fn by_slug(module: &str) -> Option<&'static DocEntry> {
    DOCS.iter().find(|e| e.slug == module)
}

/// Return every module owned by the given crate name.
fn by_crate(module: &str) -> Vec<&'static DocEntry> {
    DOCS.iter().filter(|e| e.crates.contains(&module)).collect()
}

/// Print one resolved module (TL;DR by default, full doc with `--full`).
fn print_module(entry: &DocEntry, full: bool, pretty: bool) {
    if full {
        emit(entry.body, pretty);
        return;
    }
    match tldr(entry.body) {
        Some(section) => emit(section, pretty),
        None => println!(
            "No TL;DR yet for `{}`. Run `r2e docs {} --full` for the full document.",
            entry.slug, entry.slug
        ),
    }
}

/// Resolve a module argument to its full document body — exact slug first,
/// then a crate name owning exactly one module. Returns `None` for an unknown
/// name or a crate owning several modules. Exposed for integration tests.
#[doc(hidden)]
#[allow(dead_code)] // used by integration tests, not the binary
pub fn resolve_body(module: &str) -> Option<&'static str> {
    if let Some(entry) = by_slug(module) {
        return Some(entry.body);
    }
    match by_crate(module).as_slice() {
        [entry] => Some(entry.body),
        _ => None,
    }
}

/// Every registered slug, sorted. Exposed for integration tests.
#[doc(hidden)]
#[allow(dead_code)] // used by integration tests, not the binary
pub fn slugs() -> Vec<&'static str> {
    let mut s: Vec<&str> = DOCS.iter().map(|e| e.slug).collect();
    s.sort_unstable();
    s
}

/// Entry point for `r2e docs`.
///
/// - `module == None` → list all modules.
/// - exact slug → print that module.
/// - crate name → one owned module prints it; several list them.
/// - otherwise → error listing the available slugs.
pub fn run(module: Option<&str>, full: bool, pretty: bool) -> Result<(), Box<dyn Error>> {
    let Some(module) = module else {
        print_list();
        return Ok(());
    };

    if let Some(entry) = by_slug(module) {
        print_module(entry, full, pretty);
        return Ok(());
    }

    let owned = by_crate(module);
    match owned.as_slice() {
        [] => {
            let mut slugs: Vec<&str> = DOCS.iter().map(|e| e.slug).collect();
            slugs.sort_unstable();
            Err(format!(
                "unknown module `{}`. Available: {}",
                module,
                slugs.join(", ")
            )
            .into())
        }
        [entry] => {
            print_module(entry, full, pretty);
            Ok(())
        }
        many => {
            println!("Crate `{module}` owns several modules:\n");
            for entry in many {
                println!("  {}", label(entry));
            }
            println!("\nRun `r2e docs <module>` to read one.");
            Ok(())
        }
    }
}
