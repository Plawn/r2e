use colored::Colorize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug)]
#[doc(hidden)]
pub struct Route {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub file: String,
    pub line: usize,
    pub roles: Option<String>,
}

/// List all declared routes by parsing source files.
///
/// Scans `src/controllers/*.rs` (excluding `mod.rs`) for route attributes
/// (`#[get]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]`, `#[any]`,
/// `#[sse]`, `#[ws]`, `#[fallback]`), extracts base paths from
/// `#[controller(path = "...")]`, and prints a sorted table with method,
/// path, handler name, file, and line number.
///
/// Returns an error if `src/controllers/` does not exist.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let controllers_dir = Path::new("src/controllers");
    if !controllers_dir.exists() {
        return Err("src/controllers/ directory not found".into());
    }

    // A controller listed in a `#[module(prefix = "…", controllers(...))]` is
    // mounted under that prefix, so its rows must show the mounted path.
    // Modules live anywhere under `src/`, not just in `src/controllers/`.
    let prefixes = collect_module_prefixes(Path::new("src"));

    let mut routes = Vec::new();

    for entry in fs::read_dir(controllers_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "rs")
            && path.file_name() != Some("mod.rs".as_ref())
        {
            parse_routes_from_file_with_prefixes(&path, &mut routes, &prefixes)?;
        }
    }

    if routes.is_empty() {
        println!("{}", "No routes found.".dimmed());
        return Ok(());
    }

    routes.sort_by(|a, b| a.path.cmp(&b.path));

    println!("{}", "Declared routes:".bold());
    println!();
    println!(
        "  {:<8} {:<35} {:<25} {}",
        "METHOD".dimmed(),
        "PATH".dimmed(),
        "HANDLER".dimmed(),
        "FILE".dimmed()
    );
    println!("  {}", "-".repeat(80).dimmed());

    for route in &routes {
        let method_colored = match route.method.as_str() {
            "GET" => route.method.green(),
            "POST" => route.method.blue(),
            "PUT" => route.method.yellow(),
            "DELETE" => route.method.red(),
            "PATCH" => route.method.magenta(),
            _ => route.method.normal(),
        };

        let roles_str = route.roles.as_deref().unwrap_or("");
        let handler_str = if roles_str.is_empty() {
            route.handler.clone()
        } else {
            format!("{} [{}]", route.handler, roles_str)
        };

        println!(
            "  {:<8} {:<35} {:<25} {}:{}",
            method_colored, route.path, handler_str, route.file, route.line,
        );
    }

    println!();
    println!("  {} routes total", routes.len());

    Ok(())
}

#[doc(hidden)]
pub fn parse_routes_from_file(
    path: &Path,
    routes: &mut Vec<Route>,
) -> Result<(), Box<dyn std::error::Error>> {
    parse_routes_from_file_with_prefixes(path, routes, &HashMap::new())
}

/// Same, knowing which controllers a feature module mounts under a prefix
/// (`controller struct name -> "/api/v1"`, from [`collect_module_prefixes`]).
#[doc(hidden)]
pub fn parse_routes_from_file_with_prefixes(
    path: &Path,
    routes: &mut Vec<Route>,
    module_prefixes: &HashMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let filename = path.file_name().unwrap().to_string_lossy().to_string();

    let base_path = extract_controller_path(&content).unwrap_or_default();
    let module_prefix = extract_controller_name(&content)
        .and_then(|name| module_prefixes.get(&name).cloned())
        .unwrap_or_default();
    let base_path = format!("{module_prefix}{base_path}");

    let mut current_roles: Option<String> = None;

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Detect #[roles("...")]
        if trimmed.starts_with("#[roles(") {
            current_roles = extract_string_arg(trimmed, "roles");
        }

        // Detect route macros
        for method in &["get", "post", "put", "delete", "patch", "any", "sse", "ws"] {
            let pattern = format!("#[{}(", method);
            if trimmed.starts_with(&pattern) {
                if let Some(route_path) = extract_string_arg(trimmed, method) {
                    let handler = find_next_fn_name(&content, line_num);

                    let full_path = if base_path.is_empty() {
                        route_path
                    } else if route_path == "/" {
                        base_path.clone()
                    } else {
                        format!("{}{}", base_path, route_path)
                    };

                    routes.push(Route {
                        method: method.to_uppercase(),
                        path: full_path,
                        handler: handler.unwrap_or_else(|| "?".to_string()),
                        file: filename.clone(),
                        line: line_num + 1,
                        roles: current_roles.take(),
                    });
                }
            }
        }

        // Detect #[fallback] — no path argument; it catches every unmatched request.
        if trimmed == "#[fallback]" {
            routes.push(Route {
                method: "FALLBACK".to_string(),
                // Nested under a module prefix, a fallback is prefix-scoped.
                path: if module_prefix.is_empty() {
                    "*".to_string()
                } else {
                    format!("{module_prefix}/*")
                },
                handler: find_next_fn_name(&content, line_num).unwrap_or_else(|| "?".to_string()),
                file: filename.clone(),
                line: line_num + 1,
                roles: current_roles.take(),
            });
        }

        // Reset roles if we hit a line that's not a macro attribute
        if !trimmed.starts_with('#') && !trimmed.is_empty() {
            current_roles = None;
        }
    }

    Ok(())
}

#[doc(hidden)]
pub fn extract_controller_path(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.contains("controller(") && trimmed.contains("path") {
            if let Some(start) = trimmed.find("path") {
                let rest = &trimmed[start..];
                if let Some(quote_start) = rest.find('"') {
                    let after_quote = &rest[quote_start + 1..];
                    if let Some(quote_end) = after_quote.find('"') {
                        return Some(after_quote[..quote_end].to_string());
                    }
                }
            }
        }
    }
    None
}

/// The name of the struct a `#[controller(...)]` attribute is attached to.
#[doc(hidden)]
pub fn extract_controller_name(content: &str) -> Option<String> {
    let mut seen_attr = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.contains("controller(") {
            seen_attr = true;
            continue;
        }
        if seen_attr {
            if let Some(rest) = trimmed
                .strip_prefix("pub struct ")
                .or_else(|| trimmed.strip_prefix("struct "))
                .or_else(|| trimmed.strip_prefix("pub(crate) struct "))
            {
                let end = rest
                    .find(|c: char| !c.is_alphanumeric() && c != '_')
                    .unwrap_or(rest.len());
                return Some(rest[..end].to_string());
            }
            // Other attributes may sit between the controller attribute and
            // the struct; anything else means we mis-detected.
            if !trimmed.starts_with('#') && !trimmed.is_empty() {
                seen_attr = false;
            }
        }
    }
    None
}

/// Walk `root` recursively and map every controller listed in a
/// `#[module(prefix = "…", controllers(...))]` to that prefix.
///
/// Purely textual, like the rest of `r2e routes`: the CLI never builds the
/// app. A module without a `prefix` contributes nothing.
#[doc(hidden)]
pub fn collect_module_prefixes(root: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    collect_module_prefixes_into(root, &mut out);
    out
}

fn collect_module_prefixes_into(dir: &Path, out: &mut HashMap<String, String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_module_prefixes_into(&path, out);
        } else if path.extension().map_or(false, |ext| ext == "rs") {
            if let Ok(content) = fs::read_to_string(&path) {
                parse_module_prefixes(&content, out);
            }
        }
    }
}

/// Extract `controller -> prefix` pairs from every `#[module(...)]` in a file.
#[doc(hidden)]
pub fn parse_module_prefixes(content: &str, out: &mut HashMap<String, String>) {
    let mut rest = content;
    while let Some(start) = rest.find("#[module(") {
        let attr = rest[start..].to_string();
        // The attribute may span several lines; take up to its closing `]`.
        let end = match_bracket(&attr).unwrap_or(attr.len());
        let attr = &attr[..end];

        if let Some(prefix) = attr_string_value(attr, "prefix") {
            for controller in attr_list(attr, "controllers") {
                out.insert(controller, prefix.clone());
            }
        }
        rest = &rest[start + end.max(1)..];
    }
}

/// Index just past the `]` closing the `#[` that `s` starts with.
fn match_bracket(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// `key = "value"` inside an attribute.
fn attr_string_value(attr: &str, key: &str) -> Option<String> {
    let at = attr.find(key)?;
    let rest = &attr[at + key.len()..];
    let eq = rest.find('=')?;
    // Reject `keyfoo = ` / a `=` that belongs to another key.
    if rest[..eq].trim_start().starts_with(|c: char| c.is_alphanumeric()) {
        return None;
    }
    let after = &rest[eq + 1..];
    let quote_start = after.find('"')?;
    let after_quote = &after[quote_start + 1..];
    let quote_end = after_quote.find('"')?;
    Some(after_quote[..quote_end].to_string())
}

/// The comma-separated identifiers inside `key(...)`.
fn attr_list(attr: &str, key: &str) -> Vec<String> {
    let pattern = format!("{key}(");
    let Some(at) = attr.find(&pattern) else {
        return Vec::new();
    };
    let rest = &attr[at + pattern.len()..];
    let Some(end) = rest.find(')') else {
        return Vec::new();
    };
    rest[..end]
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[doc(hidden)]
pub fn extract_string_arg(line: &str, attr_name: &str) -> Option<String> {
    let pattern = format!("#[{}(", attr_name);
    if let Some(start) = line.find(&pattern) {
        let rest = &line[start + pattern.len()..];
        if let Some(quote_start) = rest.find('"') {
            let after_quote = &rest[quote_start + 1..];
            if let Some(quote_end) = after_quote.find('"') {
                return Some(after_quote[..quote_end].to_string());
            }
        }
    }
    None
}

#[doc(hidden)]
pub fn find_next_fn_name(content: &str, from_line: usize) -> Option<String> {
    for line in content.lines().skip(from_line + 1).take(5) {
        let trimmed = line.trim();
        if trimmed.contains("fn ") {
            let fn_start = trimmed.find("fn ").map(|i| i + 3)?;
            let rest = &trimmed[fn_start..];
            let fn_end = rest.find('(').unwrap_or(rest.len());
            return Some(rest[..fn_end].to_string());
        }
    }
    None
}
