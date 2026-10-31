use std::any::TypeId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use tracing::dispatcher::WeakDispatch;
use tracing::span;
use tracing::subscriber::Interest;
use tracing::{Dispatch, Event, Metadata};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::Registry;

/// A type-erased layer over the [`Registry`] — what [`contribute_layer`]
/// takes and what the console layer is.
pub type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync + 'static>;

/// One contributed layer. Nodes are linked append-only and live in a static:
/// they are never dropped, so the pointers `downcast_raw` hands out stay valid
/// for the life of the process.
struct Node {
    layer: BoxedLayer,
    next: OnceLock<Box<Node>>,
}

static HEAD: OnceLock<Box<Node>> = OnceLock::new();
/// Serialises appends; readers never take it.
static APPEND: Mutex<()> = Mutex::new(());
/// The subscriber hosting the slot, for `on_register_dispatch` replays.
static DISPATCH: OnceLock<WeakDispatch> = OnceLock::new();
/// Set once the slot is part of a registered subscriber.
static WIRED: AtomicBool = AtomicBool::new(false);

/// Visit every contributed layer, in insertion order.
fn for_each(mut f: impl FnMut(&BoxedLayer)) {
    let mut cur = HEAD.get();
    while let Some(node) = cur {
        f(&node.layer);
        cur = node.next.get();
    }
}

/// Mark the slot as hosted by `dispatch` (the subscriber it was registered
/// in) and replay `on_register_dispatch` on the layers already contributed.
pub(crate) fn wire(dispatch: &Dispatch) {
    let _ = DISPATCH.set(dispatch.downgrade());
    WIRED.store(true, Ordering::Release);
    for_each(|l| l.on_register_dispatch(dispatch));
}

/// Handle onto the process-wide **late-layer slot**: the layers plugins
/// [contribute](contribute_layer) after the subscriber is installed.
///
/// [`try_install`](crate::try_install) wires it into R2E's subscriber, next to
/// the console layer. An application that installs its **own** subscriber and
/// still wants contributed layers (the OpenTelemetry layer of
/// `r2e-observability`, for instance) adds the slot **first**, directly on the
/// [`Registry`], so the subscriber's registration reaches it:
///
/// ```rust,ignore
/// use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
/// tracing_subscriber::registry()
///     .with(r2e_tracing::late_layers())   // first, on the Registry
///     .with(tracing_subscriber::fmt::layer())
///     .with(r2e_tracing::NoiseFilter)
///     .with(tracing_subscriber::EnvFilter::from_default_env())
///     .init();
/// ```
///
/// The slot is wired by the subscriber registration (`Dispatch::new`) that
/// sees it; put it in one subscriber only — the global one.
#[derive(Debug, Clone, Copy)]
pub struct LateLayers {
    _p: (),
}

/// The late-layer slot, to add to a hand-built subscriber — see
/// [`LateLayers`].
pub fn late_layers() -> LateLayers {
    LateLayers { _p: () }
}

/// [`contribute_layer`] was called before any subscriber hosts the late-layer
/// slot: R2E did not install the subscriber, and the application's own stack
/// does not include [`late_layers`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotWired;

impl std::fmt::Display for NotWired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "no installed tracing subscriber hosts R2E's late-layer slot: install the subscriber \
             through R2E, or add `r2e_tracing::late_layers()` first on the `Registry` of your own",
        )
    }
}

impl std::error::Error for NotWired {}

/// Add `layer` to the installed subscriber, after the fact.
///
/// The layer sees every event and span emitted from now on (it goes through
/// the global `EnvFilter` and [`NoiseFilter`](crate::NoiseFilter) like the
/// console layer). `Err(NotWired)` when no subscriber hosts the slot yet —
/// see [`LateLayers`].
///
/// Limits:
///
/// - **no per-layer filter**: a `layer.with_filter(..)` registers its filter
///   id in `Layer::on_layer`, which runs when a stack is *built* and cannot be
///   replayed on an already-built registry — a `Filtered` layer contributed
///   here would misbehave. Filter inside the layer, or rely on the global
///   `EnvFilter`. A contributed layer's own `enabled` / `event_enabled` are
///   honoured per event, for that layer only;
/// - spans opened **before** the contribution are invisible to the layer
///   (it never saw their `on_new_span`): events inside them still reach it.
pub fn contribute_layer(layer: BoxedLayer) -> Result<(), NotWired> {
    if !WIRED.load(Ordering::Acquire) {
        return Err(NotWired);
    }
    {
        let _guard = APPEND.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(dispatch) = DISPATCH.get().and_then(WeakDispatch::upgrade) {
            layer.on_register_dispatch(&dispatch);
        }
        let node = Box::new(Node {
            layer,
            next: OnceLock::new(),
        });
        match HEAD.get() {
            None => {
                let _ = HEAD.set(node);
            }
            Some(mut tail) => {
                while let Some(next) = tail.next.get() {
                    tail = next;
                }
                let _ = tail.next.set(node);
            }
        }
    }
    // Callsites registered before this layer existed cached the interest of
    // the old set; let the new layer weigh in.
    tracing::callsite::rebuild_interest_cache();
    Ok(())
}

impl Layer<Registry> for LateLayers {
    fn on_register_dispatch(&self, subscriber: &Dispatch) {
        wire(subscriber);
    }

    // `on_layer` cannot be forwarded (the children sit behind shared
    // references, and none exist yet when a stack is built) and deliberately
    // does not wire the slot: it runs when a stack is *built*, also for a
    // stack that then loses the global install — wiring there would make
    // `contribute_layer` succeed into a subscriber nobody uses.

    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        let mut interest: Option<Interest> = None;
        for_each(|l| {
            let new = l.register_callsite(metadata);
            interest = Some(match interest.take() {
                None => new,
                Some(cur) if (cur.is_sometimes() && new.is_always()) || cur.is_never() => new,
                Some(cur) => cur,
            });
        });
        match interest {
            // Never `never`: a layer contributed later must still be able to
            // see this callsite once the interest cache is rebuilt.
            Some(i) if !i.is_never() => i,
            _ => Interest::sometimes(),
        }
    }

    fn enabled(&self, _metadata: &Metadata<'_>, _ctx: Context<'_, Registry>) -> bool {
        // The slot never filters the stack; children are filtered per event
        // in `on_event`.
        true
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        // Children come and go after the hint is cached: no hint.
        None
    }

    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, Registry>) {
        for_each(|l| l.on_new_span(attrs, id, ctx.clone()));
    }

    fn on_record(&self, span: &span::Id, values: &span::Record<'_>, ctx: Context<'_, Registry>) {
        for_each(|l| l.on_record(span, values, ctx.clone()));
    }

    fn on_follows_from(&self, span: &span::Id, follows: &span::Id, ctx: Context<'_, Registry>) {
        for_each(|l| l.on_follows_from(span, follows, ctx.clone()));
    }

    fn event_enabled(&self, _event: &Event<'_>, _ctx: Context<'_, Registry>) -> bool {
        // A contributed layer must not veto the console line: its own
        // `event_enabled` is honoured for itself only, in `on_event`.
        true
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, Registry>) {
        for_each(|l| {
            if l.enabled(event.metadata(), ctx.clone()) && l.event_enabled(event, ctx.clone()) {
                l.on_event(event, ctx.clone());
            }
        });
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, Registry>) {
        for_each(|l| l.on_enter(id, ctx.clone()));
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, Registry>) {
        for_each(|l| l.on_exit(id, ctx.clone()));
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, Registry>) {
        for_each(|l| l.on_close(id.clone(), ctx.clone()));
    }

    fn on_id_change(&self, old: &span::Id, new: &span::Id, ctx: Context<'_, Registry>) {
        for_each(|l| l.on_id_change(old, new, ctx.clone()));
    }

    unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
        if id == TypeId::of::<Self>() {
            return Some(self as *const Self as *const ());
        }
        // Children live in never-dropped static nodes, so a pointer into one
        // stays valid for as long as the caller may hold it. This is what
        // keeps `Dispatch::downcast_ref` working through the slot
        // (tracing-opentelemetry's `WithContext`).
        let mut cur = HEAD.get();
        while let Some(node) = cur {
            // SAFETY: forwarding the caller's contract to the child.
            if let Some(ptr) = unsafe { node.layer.downcast_raw(id) } {
                return Some(ptr);
            }
            cur = node.next.get();
        }
        None
    }
}
