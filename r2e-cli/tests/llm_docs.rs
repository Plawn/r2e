use r2e_cli::commands::llm_docs::{
    self, front_matter, full, stamped_version, topic, without_front_matter, HUB, TOPICS, VERSION,
};
use std::collections::BTreeSet;
use std::path::Path;
use tempfile::TempDir;

/// The `llm/` directory at the repo root — the source the CLI embeds.
fn llm_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../llm")
}

#[test]
fn embedded_topics_match_llm_dir() {
    let on_disk: BTreeSet<String> = std::fs::read_dir(llm_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    let embedded: BTreeSet<String> = TOPICS.iter().map(|t| t.slug.to_string()).collect();
    assert_eq!(
        embedded, on_disk,
        "TOPICS in commands/llm_docs.rs must list exactly the llm/*.md files"
    );
    let slugs: Vec<&str> = TOPICS.iter().map(|t| t.slug).collect();
    let mut sorted = slugs.clone();
    sorted.sort_unstable();
    assert_eq!(slugs, sorted, "TOPICS is kept sorted");
}

#[test]
fn every_topic_has_front_matter_and_tldr() {
    for t in TOPICS {
        assert_eq!(front_matter(t.body, "topic"), Some(t.slug), "{}", t.slug);
        assert!(front_matter(t.body, "features").is_some_and(|f| !f.is_empty()), "{}", t.slug);
        assert!(front_matter(t.body, "tokens").is_some_and(|f| f.starts_with('~')), "{}", t.slug);
        let body = without_front_matter(t.body);
        assert!(body.starts_with("## "), "{}: body must start with its title", t.slug);
        assert!(body.contains("\n### TL;DR\n"), "{}: missing ### TL;DR", t.slug);
    }
}

#[test]
fn requires_reference_existing_topics() {
    for t in TOPICS {
        for dep in front_matter(t.body, "requires")
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            assert!(topic(dep).is_some(), "{}: requires unknown topic `{dep}`", t.slug);
        }
    }
}

#[test]
fn hub_routes_every_topic() {
    for t in TOPICS {
        assert!(
            HUB.contains(&format!("llm/{}.md", t.slug)),
            "llm.txt routing table does not mention {}",
            t.slug
        );
    }
}

#[test]
fn full_document_is_stamped_and_contains_every_topic() {
    let doc = full();
    assert_eq!(stamped_version(&doc), Some(VERSION));
    assert!(doc.contains("# R2E Framework"));
    for t in TOPICS {
        let title = without_front_matter(t.body).lines().next().unwrap();
        assert!(doc.contains(title), "full() is missing {title}");
        assert!(!doc.contains(&format!("topic: {}\n", t.slug)), "front matter leaked for {}", t.slug);
    }
}

#[test]
fn topic_lookup() {
    assert!(topic("quick-start").is_some());
    assert!(topic("nope").is_none());
}

#[test]
fn export_writes_hub_and_every_topic() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("docs/r2e");
    let n = llm_docs::export(&dir).unwrap();
    assert_eq!(n, TOPICS.len() + 1);

    let hub = std::fs::read_to_string(dir.join("llm.txt")).unwrap();
    assert_eq!(stamped_version(&hub), Some(VERSION));
    assert!(hub.contains("## Routing table"));
    for t in TOPICS {
        let body = std::fs::read_to_string(dir.join("llm").join(format!("{}.md", t.slug))).unwrap();
        assert_eq!(body, t.body, "{} exported verbatim", t.slug);
    }

    // Re-exporting over an existing copy is idempotent.
    assert_eq!(llm_docs::export(&dir).unwrap(), n);
}

#[test]
fn stamped_version_parses_only_the_stamp() {
    assert_eq!(stamped_version("<!-- R2E v0.3.0 — exported by r2e docs --llm --export -->\n# hi"), Some("0.3.0"));
    assert_eq!(stamped_version("# no stamp\n"), None);
    assert_eq!(stamped_version(""), None);
}

#[test]
fn run_rejects_unknown_topic() {
    let err = llm_docs::run(Some("nope"), false, None, false).unwrap_err();
    assert!(err.to_string().contains("unknown topic `nope`"));
}
