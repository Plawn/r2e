pub mod middleware;
pub mod project;

/// Simple template rendering: replaces `{{key}}` with value.
///
/// Each `(key, value)` pair in `vars` triggers a string replacement of
/// `{{key}}` in the template. Unknown placeholders are left as-is.
///
/// # Example
///
/// ```
/// use r2e_cli::commands::templates::render;
///
/// let result = render("Hello {{name}}!", &[("name", "World")]);
/// assert_eq!(result, "Hello World!");
/// ```
#[allow(dead_code)]
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut output = template.to_string();
    for (key, value) in vars {
        output = output.replace(&format!("{{{{{}}}}}", key), value);
    }
    output
}

/// Convert PascalCase to snake_case.
///
/// Inserts an underscore before each uppercase letter (except the first)
/// and lowercases all characters.
///
/// # Examples
///
/// ```
/// use r2e_cli::commands::templates::to_snake_case;
///
/// assert_eq!(to_snake_case("UserController"), "user_controller");
/// assert_eq!(to_snake_case("User"), "user");
/// ```
///
/// Note: consecutive uppercase letters are each separated individually:
/// `"HTTPClient"` becomes `"h_t_t_p_client"`.
pub fn to_snake_case(name: &str) -> String {
    let mut result = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_lowercase().next().unwrap());
        } else {
            result.push(c);
        }
    }
    result
}

/// Convert snake_case to PascalCase.
///
/// Splits on `_`, capitalizes the first letter of each segment,
/// and joins them back together.
///
/// # Examples
///
/// ```
/// use r2e_cli::commands::templates::to_pascal_case;
///
/// assert_eq!(to_pascal_case("user_service"), "UserService");
/// assert_eq!(to_pascal_case("user"), "User");
/// ```
#[allow(dead_code)]
pub fn to_pascal_case(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Compute a plural form using simple English rules.
///
/// - Words ending in `s`, `sh`, or `ch` get `es` appended.
/// - Words ending in `y` (but not `ey`, `ay`, `oy`) replace `y` with `ies`.
/// - All other words get `s` appended.
///
/// # Examples
///
/// ```
/// use r2e_cli::commands::templates::pluralize;
///
/// assert_eq!(pluralize("user"), "users");
/// assert_eq!(pluralize("category"), "categories");
/// assert_eq!(pluralize("status"), "statuses");
/// ```
pub fn pluralize(name: &str) -> String {
    if name.ends_with('s') || name.ends_with("sh") || name.ends_with("ch") {
        format!("{name}es")
    } else if name.ends_with('y')
        && !name.ends_with("ey")
        && !name.ends_with("ay")
        && !name.ends_with("oy")
    {
        format!("{}ies", &name[..name.len() - 1])
    } else {
        format!("{name}s")
    }
}
