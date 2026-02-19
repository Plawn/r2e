use r2e_cli::commands::templates::{pluralize, render, to_pascal_case, to_snake_case};

// ── to_snake_case ───────────────────────────────────────────────────

#[test]
fn to_snake_case_basic() {
    assert_eq!(to_snake_case("UserController"), "user_controller");
}

#[test]
fn to_snake_case_already_snake() {
    assert_eq!(to_snake_case("user_service"), "user_service");
}

#[test]
fn to_snake_case_single_word() {
    assert_eq!(to_snake_case("User"), "user");
}

#[test]
fn to_snake_case_acronym() {
    // Each uppercase letter gets a preceding underscore
    assert_eq!(to_snake_case("HTTPClient"), "h_t_t_p_client");
}

#[test]
fn to_snake_case_lowercase() {
    assert_eq!(to_snake_case("hello"), "hello");
}

#[test]
fn to_snake_case_empty() {
    assert_eq!(to_snake_case(""), "");
}

// ── to_pascal_case ──────────────────────────────────────────────────

#[test]
fn to_pascal_case_basic() {
    assert_eq!(to_pascal_case("user_service"), "UserService");
}

#[test]
fn to_pascal_case_already_pascal() {
    // No underscores → single word, first char capitalized
    assert_eq!(to_pascal_case("UserService"), "UserService");
}

#[test]
fn to_pascal_case_single_word() {
    assert_eq!(to_pascal_case("user"), "User");
}

#[test]
fn to_pascal_case_multiple_words() {
    assert_eq!(to_pascal_case("my_cool_service"), "MyCoolService");
}

#[test]
fn to_pascal_case_empty() {
    assert_eq!(to_pascal_case(""), "");
}

// ── pluralize ───────────────────────────────────────────────────────

#[test]
fn pluralize_regular() {
    assert_eq!(pluralize("user"), "users");
}

#[test]
fn pluralize_y_ending() {
    assert_eq!(pluralize("category"), "categories");
}

#[test]
fn pluralize_s_ending() {
    assert_eq!(pluralize("status"), "statuses");
}

#[test]
fn pluralize_sh_ending() {
    assert_eq!(pluralize("crash"), "crashes");
}

#[test]
fn pluralize_ch_ending() {
    assert_eq!(pluralize("match"), "matches");
}

#[test]
fn pluralize_ey_ending_no_ies() {
    // "ey" ending → just add "s", not "ies"
    assert_eq!(pluralize("monkey"), "monkeys");
}

#[test]
fn pluralize_ay_ending_no_ies() {
    assert_eq!(pluralize("day"), "days");
}

#[test]
fn pluralize_oy_ending_no_ies() {
    assert_eq!(pluralize("boy"), "boys");
}

#[test]
fn pluralize_already_plural_s() {
    // "users" ends with 's' → "userses" (no special handling)
    assert_eq!(pluralize("users"), "userses");
}

// ── render ──────────────────────────────────────────────────────────

#[test]
fn render_basic() {
    assert_eq!(
        render("Hello {{name}}", &[("name", "World")]),
        "Hello World"
    );
}

#[test]
fn render_multiple_placeholders() {
    let result = render(
        "{{greeting}} {{name}}!",
        &[("greeting", "Hello"), ("name", "World")],
    );
    assert_eq!(result, "Hello World!");
}

#[test]
fn render_missing_placeholder() {
    // Unknown placeholders are left as-is
    assert_eq!(
        render("Hello {{unknown}}", &[("name", "World")]),
        "Hello {{unknown}}"
    );
}

#[test]
fn render_empty_vars() {
    assert_eq!(render("Hello {{name}}", &[]), "Hello {{name}}");
}

#[test]
fn render_no_placeholders() {
    assert_eq!(render("Hello World", &[("name", "X")]), "Hello World");
}

#[test]
fn render_repeated_placeholder() {
    assert_eq!(
        render("{{x}} and {{x}}", &[("x", "ok")]),
        "ok and ok"
    );
}
