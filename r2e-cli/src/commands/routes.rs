use colored::Colorize;
use std::fs;
use std::path::Path;

#[derive(Debug)]
struct Route {
    method: String,
    path: String,
    handler: String,
    file: String,
    line: usize,
    roles: Option<String>,
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let controllers_dir = Path::new("src/controllers");
    if !controllers_dir.exists() {
        return Err("src/controllers/ directory not found".into());
    }

    let mut routes = Vec::new();

    for entry in fs::read_dir(controllers_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "rs")
            && path.file_name() != Some("mod.rs".as_ref())
        {
            parse_routes_from_file(&path, &mut routes)?;
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

fn parse_routes_from_file(
    path: &Path,
    routes: &mut Vec<Route>,
) -> Result<(), Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let filename = path.file_name().unwrap().to_string_lossy().to_string();

    let base_path = extract_controller_path(&content).unwrap_or_default();

    let mut current_roles: Option<String> = None;

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Detect #[roles("...")]
        if trimmed.starts_with("#[roles(") {
            current_roles = extract_string_arg(trimmed, "roles");
        }

        // Detect route macros
        for method in &["get", "post", "put", "delete", "patch"] {
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

        // Reset roles if we hit a line that's not a macro attribute
        if !trimmed.starts_with('#') && !trimmed.is_empty() {
            current_roles = None;
        }
    }

    Ok(())
}

fn extract_controller_path(content: &str) -> Option<String> {
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

fn extract_string_arg(line: &str, attr_name: &str) -> Option<String> {
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

fn find_next_fn_name(content: &str, from_line: usize) -> Option<String> {
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
