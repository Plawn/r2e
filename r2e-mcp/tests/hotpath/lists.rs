//! Task #994 — `tools/list` must not re-allocate the macro-emitted metadata.
//!
//! `Family::visible_list` hands out a clone of the `Vec<Tool>` built at boot
//! (and `Family::wire` clones one element per `tools/call`). rmcp's `Tool`
//! stores `name`/`description` as `Cow<'static, str>`, so a literal emitted
//! by `#[tool]` can travel borrowed all the way onto the wire: the clone is
//! then one `Vec` allocation, whatever the tools say about themselves.
//!
//! The guard is exact rather than statistical — a single `Vec` allocation of
//! `n * size_of::<Tool>()` bytes — because the regression is exact too: one
//! `String` per description, per tool, per request.

use std::borrow::Cow;
use std::sync::Arc;

use r2e_core::beans::BeanContext;
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{McpService, Params};
use rmcp::model::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::counter::{measure, runtime, steady_state};

/// One long description, spelled out at both call sites: a `String`-typed
/// description would show up as ~200 bytes per tool per request, an order of
/// magnitude above the slack any guard here allows.
const LONG: &str = "Perform the operation and return its result. This description is deliberately \
                    long: it is the payload a per-request `String` clone would copy, so the guard \
                    below can tell a borrowed literal from an owned one by weight alone rather \
                    than by counting allocations of an incidental size.";

#[derive(Deserialize, JsonSchema, ObjectParams)]
pub struct Operands {
    /// Left operand.
    pub a: f64,
    /// Right operand.
    pub b: f64,
}

#[derive(Serialize, JsonSchema)]
pub struct Sum {
    /// The result.
    pub value: f64,
}

/// Six documented tools, short descriptions.
#[controller]
struct ShortDocs;

#[mcp_routes]
impl ShortDocs {
    /// Add.
    #[tool(read_only, idempotent)]
    async fn add(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum { value: p.a + p.b })
    }

    /// Sub.
    #[tool(read_only)]
    async fn sub(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum { value: p.a - p.b })
    }

    /// Mul.
    #[tool(read_only)]
    async fn mul(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum { value: p.a * p.b })
    }

    /// Div.
    #[tool(read_only)]
    async fn div(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum { value: p.a / p.b })
    }

    /// Min.
    #[tool(read_only)]
    async fn min(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum {
            value: p.a.min(p.b),
        })
    }

    /// Max.
    #[tool(read_only)]
    async fn max(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum {
            value: p.a.max(p.b),
        })
    }
}

/// The same six tools, each with a long description.
#[controller]
struct LongDocs;

#[mcp_routes]
impl LongDocs {
    #[tool(read_only, idempotent, description = "Perform the operation and return its result. This description is deliberately long: it is the payload a per-request `String` clone would copy, so the guard below can tell a borrowed literal from an owned one by weight alone rather than by counting allocations of an incidental size.")]
    async fn add(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum { value: p.a + p.b })
    }

    #[tool(read_only, description = "Perform the operation and return its result. This description is deliberately long: it is the payload a per-request `String` clone would copy, so the guard below can tell a borrowed literal from an owned one by weight alone rather than by counting allocations of an incidental size.")]
    async fn sub(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum { value: p.a - p.b })
    }

    #[tool(read_only, description = "Perform the operation and return its result. This description is deliberately long: it is the payload a per-request `String` clone would copy, so the guard below can tell a borrowed literal from an owned one by weight alone rather than by counting allocations of an incidental size.")]
    async fn mul(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum { value: p.a * p.b })
    }

    #[tool(read_only, description = "Perform the operation and return its result. This description is deliberately long: it is the payload a per-request `String` clone would copy, so the guard below can tell a borrowed literal from an owned one by weight alone rather than by counting allocations of an incidental size.")]
    async fn div(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum { value: p.a / p.b })
    }

    #[tool(read_only, description = "Perform the operation and return its result. This description is deliberately long: it is the payload a per-request `String` clone would copy, so the guard below can tell a borrowed literal from an owned one by weight alone rather than by counting allocations of an incidental size.")]
    async fn min(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum {
            value: p.a.min(p.b),
        })
    }

    #[tool(read_only, description = "Perform the operation and return its result. This description is deliberately long: it is the payload a per-request `String` clone would copy, so the guard below can tell a borrowed literal from an owned one by weight alone rather than by counting allocations of an incidental size.")]
    async fn max(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum {
            value: p.a.max(p.b),
        })
    }
}

const TOOLS: usize = 6;

fn context(rt: &r2e_core::rt::Runtime) -> Arc<BeanContext> {
    rt.block_on(async { AppBuilder::new().build_state().await.bean_context().clone() })
}

/// The `Vec<Tool>` the handler builds once at boot and clones per
/// `tools/list`.
fn wire_list<S: McpService>(ctx: &Arc<BeanContext>) -> Vec<Tool> {
    let routes = S::routes(ctx);
    assert_eq!(routes.tools.len(), TOOLS);
    routes.tools.iter().map(|t| t.to_rmcp_tool()).collect()
}

/// Exactly one allocation — the destination `Vec` — and nothing else, for a
/// list of six tools with descriptions, input schemas, output schemas and
/// annotations.
#[test]
fn cloning_the_tools_list_allocates_only_the_vec() {
    let rt = runtime();
    let ctx = context(&rt);
    let list = wire_list::<ShortDocs>(&ctx);

    // Warm up (the first clone may touch a lazily-initialised allocator bin).
    drop(list.clone());
    let (clone, alloc) = measure(|| list.clone());
    drop(clone);

    eprintln!("[hotpath] tools/list clone of {TOOLS} tools: {alloc}");
    assert_eq!(
        alloc.count, 1,
        "cloning the prebuilt tools/list payload allocated {} times for {TOOLS} tools — \
         the wire metadata is owned, not borrowed. See docs/claude/hot-path-clone-audit.md.",
        alloc.count,
    );
    assert_eq!(
        alloc.bytes,
        (TOOLS * std::mem::size_of::<Tool>()) as u64,
        "the only allocation must be the destination Vec itself",
    );
}

/// The same clone, tool for tool, with descriptions an order of magnitude
/// longer: identical cost. A `String` description would add one allocation
/// and a few hundred bytes per tool, per request.
#[test]
fn the_clone_cost_does_not_scale_with_the_description_length() {
    let rt = runtime();
    let ctx = context(&rt);

    let short = wire_list::<ShortDocs>(&ctx);
    let long = wire_list::<LongDocs>(&ctx);
    assert_eq!(
        long[0].description.as_deref(),
        Some(LONG),
        "fixture drift: the long-description service must carry LONG verbatim",
    );

    let short_cost = steady_state(50, || drop(short.clone()));
    let long_cost = steady_state(50, || drop(long.clone()));

    eprintln!("[hotpath] tools/list clone: short docs = {short_cost}; long docs = {long_cost}");
    assert_eq!(
        short_cost, long_cost,
        "cloning the tools/list payload costs more when the descriptions are longer — \
         they are being re-allocated per request instead of borrowed",
    );
}

/// The structural reason the guards above hold: the macro emits literals, so
/// every name and description in the wire list is `Cow::Borrowed`.
#[test]
fn macro_emitted_tool_metadata_is_borrowed() {
    let rt = runtime();
    let ctx = context(&rt);

    for tool in wire_list::<LongDocs>(&ctx) {
        assert!(
            matches!(tool.name, Cow::Borrowed(_)),
            "tool name `{}` is owned",
            tool.name,
        );
        assert!(
            matches!(tool.description, Some(Cow::Borrowed(_))),
            "description of `{}` is owned — `#[tool]` must emit a borrowed literal",
            tool.name,
        );
    }
}
