//! Doctest host for the agent-facing reference under `llm/`. Nothing here is
//! meant to be used; see `build.rs` for how the topics become module docs.
#![doc(html_no_source)]

include!(concat!(env!("OUT_DIR"), "/topics.rs"));

/// Per-topic scaffolding for the snippets: the placeholder services, entities
/// and apps a `llm/<topic>.md` block names without defining. Every block of a
/// topic starts with a hidden `use llm_doctests::fixtures::<topic>::*;`.
/// One file per topic under `src/fixtures/`; a topic without one gets an
/// empty module.
pub mod fixtures {
    include!(concat!(env!("OUT_DIR"), "/fixtures.rs"));
}
