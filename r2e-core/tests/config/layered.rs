//! `Layered<C>` and the derived `ConfigProperties::overlay`: explicit > file >
//! preset, field by field, recursing into sections.

use r2e_core::config::{ConfigProperties, Layered, R2eConfig};

#[derive(r2e_macros::ConfigProperties, Clone, Debug, Default, PartialEq)]
pub struct Limits {
    pub max: Option<i64>,
    pub burst: Option<i64>,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug, Default, PartialEq)]
pub struct Extra {
    pub max: Option<i64>,
    pub burst: Option<i64>,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug, Default, PartialEq)]
pub struct Knobs {
    pub name: Option<String>,
    pub paths: Option<Vec<String>>,
    pub enabled: Option<bool>,
    #[config(section)]
    pub limits: Limits,
    #[config(section)]
    pub extra: Option<Extra>,
}

/// A plain field has no "unset": such a config is not layerable.
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
pub struct Plain {
    #[config(default = false)]
    pub enabled: bool,
    pub name: Option<String>,
}

/// A section of a non-layerable type makes the parent non-layerable too.
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
pub struct WrapsPlain {
    pub name: Option<String>,
    #[config(section)]
    pub plain: Plain,
}

/// An *optional* section of a non-layerable type is still layerable: `None`
/// is its "unset", and a present one is replaced as a whole.
#[derive(r2e_macros::ConfigProperties, Clone, Debug, Default)]
pub struct WrapsOptionalPlain {
    pub name: Option<String>,
    #[config(section)]
    pub required: Option<Required>,
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug, PartialEq)]
pub struct Required {
    pub issuer: String,
    pub audience: Option<String>,
}

fn knobs(name: &str) -> Knobs {
    Knobs {
        name: Some(name.into()),
        ..Knobs::default()
    }
}

#[test]
fn layerable_is_derived_from_the_field_shapes() {
    const { assert!(Limits::LAYERABLE) };
    const { assert!(Knobs::LAYERABLE) };
    const { assert!(!Plain::LAYERABLE) };
    const { assert!(!WrapsPlain::LAYERABLE) };
    const { assert!(!Required::LAYERABLE) };
    const { assert!(WrapsOptionalPlain::LAYERABLE) };
}

#[test]
fn overlay_keeps_the_upper_value_and_fills_the_holes() {
    let upper = Knobs {
        name: Some("upper".into()),
        enabled: Some(false),
        ..Knobs::default()
    };
    let under = Knobs {
        name: Some("under".into()),
        paths: Some(vec!["/under".into()]),
        enabled: Some(true),
        ..Knobs::default()
    };
    let merged = upper.overlay(under);
    assert_eq!(merged.name.as_deref(), Some("upper"));
    assert_eq!(merged.paths, Some(vec!["/under".to_string()]));
    assert_eq!(merged.enabled, Some(false), "an explicit `false` is a value, not a hole");
}

#[test]
fn overlay_replaces_lists_instead_of_concatenating() {
    let upper = Knobs {
        paths: Some(vec!["/a".into()]),
        ..Knobs::default()
    };
    let under = Knobs {
        paths: Some(vec!["/b".into(), "/c".into()]),
        ..Knobs::default()
    };
    assert_eq!(upper.overlay(under).paths, Some(vec!["/a".to_string()]));
}

#[test]
fn overlay_recurses_into_sections() {
    let upper = Knobs {
        limits: Limits { max: Some(1), burst: None },
        extra: Some(Extra { max: None, burst: Some(7) }),
        ..Knobs::default()
    };
    let under = Knobs {
        limits: Limits { max: Some(100), burst: Some(200) },
        extra: Some(Extra { max: Some(300), burst: Some(400) }),
        ..Knobs::default()
    };
    let merged = upper.overlay(under);
    assert_eq!(merged.limits, Limits { max: Some(1), burst: Some(200) });
    assert_eq!(merged.extra, Some(Extra { max: Some(300), burst: Some(7) }));
}

#[test]
fn an_absent_optional_section_falls_through() {
    let under = Knobs {
        extra: Some(Extra { max: Some(3), burst: None }),
        ..Knobs::default()
    };
    assert_eq!(
        Knobs::default().overlay(under.clone()).extra,
        under.extra,
        "None above: the lower section is kept whole"
    );
    assert_eq!(under.clone().overlay(Knobs::default()).extra, under.extra);
}

#[test]
fn a_non_layerable_optional_section_is_replaced_whole() {
    let section = |issuer: &str, audience: Option<&str>| {
        Some(Required {
            issuer: issuer.into(),
            audience: audience.map(Into::into),
        })
    };
    let upper = WrapsOptionalPlain { name: None, required: section("upper", None) };
    let under = WrapsOptionalPlain { name: Some("n".into()), required: section("under", Some("aud")) };

    let merged = upper.overlay(under.clone());
    assert_eq!(merged.name.as_deref(), Some("n"));
    assert_eq!(
        merged.required,
        section("upper", None),
        "no field-wise merge: `audience` does not leak from the lower tier"
    );
    assert_eq!(WrapsOptionalPlain::default().overlay(under.clone()).required, under.required);
}

#[test]
fn resolve_orders_explicit_then_file_then_preset() {
    let layered = || {
        let mut l = Layered::with_preset(Knobs {
            name: Some("preset".into()),
            paths: Some(vec!["/preset".into()]),
            enabled: Some(true),
            ..Knobs::default()
        });
        l.set(|c| c.name = Some("explicit".into()));
        l
    };
    let file = Knobs {
        name: Some("file".into()),
        paths: Some(vec!["/file".into()]),
        ..Knobs::default()
    };

    let all = layered().resolve(Some(file));
    assert_eq!(all.name.as_deref(), Some("explicit"));
    assert_eq!(all.paths, Some(vec!["/file".to_string()]));
    assert_eq!(all.enabled, Some(true), "only the preset set it");

    let no_file = layered().resolve(None);
    assert_eq!(no_file.name.as_deref(), Some("explicit"));
    assert_eq!(no_file.paths, Some(vec!["/preset".to_string()]));

    assert_eq!(Layered::<Knobs>::new().resolve(None), Knobs::default());
}

#[test]
fn preset_setters_never_outrank_the_file() {
    let mut l = Layered::<Knobs>::new();
    l.set_preset(|c| c.name = Some("preset".into()));
    assert_eq!(l.preset().name.as_deref(), Some("preset"));
    assert_eq!(l.explicit().name, None);
    assert_eq!(l.clone().resolve(Some(knobs("file"))).name.as_deref(), Some("file"));

    l.replace_preset(knobs("replaced"));
    assert_eq!(l.resolve(None).name.as_deref(), Some("replaced"));
}

#[test]
fn the_file_tier_is_the_loaded_section() {
    let config = R2eConfig::from_yaml_str(
        r#"
knobs:
  name: from-yaml
  limits:
    burst: 9
"#,
    )
    .unwrap();
    let file = Knobs::from_config(&config, Some("knobs")).unwrap();

    let mut l = Layered::with_preset(Knobs {
        limits: Limits { max: Some(50), burst: Some(60) },
        ..knobs("preset")
    });
    l.set(|c| c.limits.max = Some(1));

    let cfg = l.resolve(Some(file));
    assert_eq!(cfg.name.as_deref(), Some("from-yaml"));
    assert_eq!(cfg.limits, Limits { max: Some(1), burst: Some(9) });
}
