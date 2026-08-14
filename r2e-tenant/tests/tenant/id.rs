//! `TenantId` validation.
//!
//! The charset is a security boundary, not a style choice: a tenant id ends up
//! in database names, schema names, file prefixes and cache keys, so the cases
//! below (traversal, NUL, separators, casing, length) are the ones that must
//! never produce a `TenantId`.

use std::collections::HashMap;

use r2e_tenant::{InvalidTenantId, TenantId, MAX_TENANT_ID_LEN};

#[test]
fn accepts_the_documented_shape() {
    for raw in [
        "acme",
        "acme-eu",
        "acme_eu",
        "acme.eu",
        "a",
        "0",
        "tenant-42",
        "a1._-b",
        &"a".repeat(MAX_TENANT_ID_LEN),
    ] {
        assert!(
            TenantId::parse(raw).is_ok(),
            "expected `{raw}` to be a valid tenant id"
        );
    }
}

#[test]
fn rejects_hostile_ids() {
    // Each of these, if accepted, reaches a path, a SQL identifier, or a header.
    let cases: [(&str, &str); 12] = [
        ("", "empty"),
        ("../etc/passwd", "path traversal"),
        ("..", "parent directory"),
        ("a/b", "path separator"),
        ("a\\b", "windows separator"),
        ("a\0b", "NUL byte"),
        ("a b", "space"),
        ("Acme", "uppercase"),
        ("aCme", "inner uppercase"),
        ("a%00", "percent encoding"),
        ("tenant;drop table", "sql punctuation"),
        ("a\nb", "newline"),
    ];
    for (raw, why) in cases {
        assert!(
            TenantId::parse(raw).is_err(),
            "expected `{raw}` ({why}) to be rejected"
        );
    }
}

#[test]
fn rejects_leading_separators() {
    // A leading dot/dash/underscore is what turns an id into `-flag`, `.hidden`
    // or a relative path component.
    for raw in [".acme", "-acme", "_acme"] {
        assert_eq!(
            TenantId::parse(raw),
            Err(InvalidTenantId::InvalidStart(raw.chars().next().unwrap())),
            "expected `{raw}` to be rejected for its first character"
        );
    }
}

#[test]
fn rejects_ids_over_the_maximum_length() {
    let too_long = "a".repeat(MAX_TENANT_ID_LEN + 1);
    assert_eq!(
        TenantId::parse(&too_long),
        Err(InvalidTenantId::TooLong(MAX_TENANT_ID_LEN + 1))
    );
    assert!(TenantId::parse(&"a".repeat(MAX_TENANT_ID_LEN)).is_ok());
}

#[test]
fn reports_which_character_was_invalid() {
    assert_eq!(
        TenantId::parse("acme!"),
        Err(InvalidTenantId::InvalidChar('!'))
    );
    assert_eq!(TenantId::parse(""), Err(InvalidTenantId::Empty));
}

#[test]
fn error_messages_name_the_rule() {
    let err = TenantId::parse("Acme").unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("lowercase"),
        "unhelpful message: {message}"
    );

    let err = TenantId::parse(&"a".repeat(99)).unwrap_err();
    assert!(err.to_string().contains("63"), "{err}");
}

#[test]
fn parse_owned_validates_too() {
    assert!(TenantId::parse_owned("acme".to_string()).is_ok());
    assert!(TenantId::parse_owned("../x".to_string()).is_err());
}

#[test]
fn conversions_go_through_validation() {
    assert!("acme".parse::<TenantId>().is_ok());
    assert!("../x".parse::<TenantId>().is_err());
    assert!(TenantId::try_from("acme").is_ok());
    assert!(TenantId::try_from("../x".to_string()).is_err());
}

#[test]
fn displays_as_the_bare_id_and_debugs_wrapped() {
    let id = TenantId::parse("acme").unwrap();
    assert_eq!(id.to_string(), "acme");
    assert_eq!(format!("{id:?}"), "TenantId(acme)");
    assert_eq!(id.as_str(), "acme");
    assert_eq!(AsRef::<str>::as_ref(&id), "acme");
}

#[test]
fn serializes_as_a_plain_string() {
    let id = TenantId::parse("acme-eu").unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"acme-eu\"");
}

#[test]
fn works_as_a_map_key_and_borrows_as_str() {
    let mut map: HashMap<TenantId, u8> = HashMap::new();
    map.insert(TenantId::parse("acme").unwrap(), 1);
    // `Borrow<str>` means a lookup needs no allocation.
    assert_eq!(map.get("acme"), Some(&1));
    assert_eq!(
        map.get(&TenantId::parse("acme").unwrap()),
        Some(&1),
        "two parses of the same id must be the same key"
    );
}

#[test]
fn orders_lexicographically() {
    let mut ids = [
        TenantId::parse("beta").unwrap(),
        TenantId::parse("acme").unwrap(),
    ];
    ids.sort();
    assert_eq!(ids[0].as_str(), "acme");
}
