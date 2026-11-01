//! The boot forwarder: logs emitted before [`install`](crate::install) are
//! buffered, then replayed through the real stack.
//!
//! # Model
//!
//! The `tracing` global default is one-shot, but the subscriber R2E installs
//! is only known once the builder has read the `tracing:` section and every
//! plugin has contributed its layers — after config loading, `App::setup` and
//! plugin setup have already logged. So [`capture_boot_logs`] installs a
//! [`Forwarder`] as the global default first, and [`install`](crate::install)
//! hands it the real stack later ([`take_over`]). Quarkus does the same with
//! its `DelayedHandler`.
//!
//! The forwarder has three states:
//!
//! - **Buffering** (initial): the events the boot filter (below) enables are
//!   rendered to strings and stored (at most [`BUFFER_CAP`]; the rest is
//!   counted and reported by one WARN after the replay). Spans get ids from a
//!   reserved range and are not stored.
//! - **Forwarding** (after [`take_over`]): every `Subscriber` call goes 1:1 to
//!   the real stack — one atomic load (the target's `OnceLock`) and a deref.
//!   The buffer is replayed in order right after the switch, and the callsite
//!   interest cache is rebuilt (interest and `LevelFilter::current()` were
//!   computed against the buffering hints). Each replayed event goes through
//!   the real stack's `enabled` / `event_enabled` first, so the installed
//!   filter (`RUST_LOG` or `tracing.filter`) and any
//!   [`Suppress`](crate::Suppress) still decide what is printed.
//! - **Disabled** ([`discard_boot_logs`], `tracing.enabled: false`): every
//!   callsite is `never`, the buffer is dropped. A later `install` still
//!   switches to Forwarding.
//!
//! # Boot filter
//!
//! What the buffer keeps is decided by `RUST_LOG`, read once when the
//! forwarder is put in place: it is the only filter source available that
//! early (`tracing.filter` is not loaded yet), and the one [`install`](crate::install)
//! gives priority to — so with `RUST_LOG` set, the buffer is filtered by the
//! same directives as the final subscriber. Unset or invalid, the buffer keeps INFO
//! and above. The full directive set is honoured (`info,my_app=debug` buffers
//! `my_app`'s DEBUG lines, not hyper's), by evaluating an `EnvFilter` over a
//! bare `Registry`: only its static directives can match, since the boot
//! filter never sees a span — span-scoped directives (`[boot]=debug`) buffer
//! nothing beyond the static ones. That loses nothing the replay could have
//! printed: a replayed event has no parent span, so such a directive would
//! not match it there either.
//!
//! # Reserved span ids
//!
//! A span opened while buffering has an id the real `Registry` has never seen,
//! and `Registry::try_close` / `clone_span` panic on an unknown id — an
//! `#[instrument]`ed `App::build` straddles the install. Such ids are tracked
//! (with their reference count) until their last handle closes, and
//! `enter`/`exit`/`record`/`record_follows_from`/`clone_span`/`try_close` on
//! one are swallowed after the switch; a span created after the switch with
//! one as its explicit parent gets the contextual parent instead. The check
//! costs one more atomic load on span operations, and a lock only while such a
//! span is alive *and* the id falls in the reserved range. (Membership is
//! exact rather than "id ≥ base": `Registry` ids carry the slab generation in
//! their high bits, so a long-lived process does produce real ids above any
//! fixed base.)
//!
//! # Limits
//!
//! - Replayed events are timestamped at replay time, not emission time (the
//!   gap is `setup` + plugin setup, typically milliseconds — but seconds when
//!   the boot waits on an upstream with retries). Primitive field values keep
//!   their type; any other value is replayed as its `Debug` rendering,
//!   captured at emission.
//! - Events only, filtered by the boot filter: spans are lost — a replayed
//!   event has no parent span and no [`LogFields`](crate::LogFields).
//! - A replayed event keeps at most [`MAX_FIELDS`] fields.

use std::any::TypeId;
use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use tracing::field::{DisplayValue, Field, Value, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{self, Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Dispatch, Event, Metadata, Subscriber};
use tracing_core::span::Current;
use tracing_subscriber::layer::{Layered, SubscriberExt};
use tracing_subscriber::{EnvFilter, Registry};

/// The real stack; set once by [`take_over`]. Its presence **is** the
/// Forwarding state — the hot path reads nothing else.
static TARGET: OnceLock<Arc<dyn Subscriber + Send + Sync>> = OnceLock::new();
/// The global dispatch wrapping the forwarder, handed to the target's
/// `on_register_dispatch` on take-over.
static GLOBAL: OnceLock<Dispatch> = OnceLock::new();
/// Whether the forwarder won the global default (`None`: not tried yet).
static OURS: OnceLock<bool> = OnceLock::new();
/// Buffering vs Disabled, while there is no target.
static DISABLED: AtomicBool = AtomicBool::new(false);
/// Buffered events. Also serialises the switch to Forwarding: an event is
/// pushed only after re-checking, under this lock, that there is no target.
static BUFFER: Mutex<Vec<Stored>> = Mutex::new(Vec::new());
/// Events discarded because the buffer was full.
static DROPPED: AtomicUsize = AtomicUsize::new(0);
/// Next reserved span id, offset from [`RESERVED_BASE`].
static NEXT_SPAN: AtomicU64 = AtomicU64::new(1);
/// Live reserved span ids and their reference counts.
static RESERVED: Mutex<Option<HashMap<u64, usize>>> = Mutex::new(None);
/// `RESERVED.len()`, readable without the lock.
static LIVE_RESERVED: AtomicUsize = AtomicUsize::new(0);
/// What the buffer keeps — see the module docs. Built on first use, which
/// [`ensure_forwarder`] forces before the forwarder is registered.
static BOOT_FILTER: OnceLock<BootFilter> = OnceLock::new();

/// Start of the span-id range handed out while buffering.
const RESERVED_BASE: u64 = 1 << 62;
/// The most events kept before the install.
pub(crate) const BUFFER_CAP: usize = 1024;
/// The most fields a replayed event keeps.
pub(crate) const MAX_FIELDS: usize = 32;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One buffered event.
struct Stored {
    metadata: &'static Metadata<'static>,
    fields: Vec<(Field, Val)>,
}

/// A field value, owned. Primitives keep their type (the replay renders them
/// exactly as the original event would); anything else is rendered once,
/// through `Debug` (or `Display` for errors), and replayed unquoted.
enum Val {
    Str(String),
    I64(i64),
    U64(u64),
    I128(i128),
    U128(u128),
    F64(f64),
    Bool(bool),
    Rendered(DisplayValue<String>),
}

impl Val {
    fn as_value(&self) -> &dyn Value {
        match self {
            Val::Str(v) => v,
            Val::I64(v) => v,
            Val::U64(v) => v,
            Val::I128(v) => v,
            Val::U128(v) => v,
            Val::F64(v) => v,
            Val::Bool(v) => v,
            Val::Rendered(v) => v,
        }
    }
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Val::Str(v) => f.write_str(v),
            Val::I64(v) => v.fmt(f),
            Val::U64(v) => v.fmt(f),
            Val::I128(v) => v.fmt(f),
            Val::U128(v) => v.fmt(f),
            Val::F64(v) => v.fmt(f),
            Val::Bool(v) => v.fmt(f),
            // `DisplayValue`'s `Debug` is its inner `Display`.
            Val::Rendered(v) => fmt::Debug::fmt(v, f),
        }
    }
}

/// Collects every field of an event as a [`Val`].
struct Render(Vec<(Field, Val)>);

impl Render {
    fn push(&mut self, field: &Field, value: Val) {
        self.0.push((field.clone(), value));
    }
}

impl Visit for Render {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field, Val::Str(value.to_owned()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field, Val::I64(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field, Val::U64(value));
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        self.push(field, Val::I128(value));
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        self.push(field, Val::U128(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.push(field, Val::F64(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field, Val::Bool(value));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.push(
            field,
            Val::Rendered(tracing::field::display(value.to_string())),
        );
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.push(
            field,
            Val::Rendered(tracing::field::display(format!("{value:?}"))),
        );
    }
}

// ── boot filter ──────────────────────────────────────────────────────────────

/// `RUST_LOG` (or `info`), evaluable on its own: `EnvFilter::enabled` needs a
/// layer `Context`, which only a `Layered` stack can build, so the filter sits
/// on a bare `Registry` that sees nothing but `enabled` calls.
struct BootFilter {
    stack: Layered<EnvFilter, Registry>,
    hint: Option<LevelFilter>,
}

fn boot_filter() -> &'static BootFilter {
    BOOT_FILTER.get_or_init(|| {
        // Same source and precedence as `install`: a valid `RUST_LOG` is what
        // the final subscriber will use; otherwise `tracing.filter` will, and
        // it is not known yet.
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        BootFilter {
            hint: filter.max_level_hint(),
            stack: Registry::default().with(filter),
        }
    })
}

// ── reserved span ids ────────────────────────────────────────────────────────

fn reserve() -> Id {
    let n = NEXT_SPAN.fetch_add(1, Ordering::Relaxed);
    let id = RESERVED_BASE + n;
    let mut map = lock(&RESERVED);
    map.get_or_insert_with(HashMap::new).insert(id, 1);
    LIVE_RESERVED.fetch_add(1, Ordering::Release);
    Id::from_u64(id)
}

/// Whether `id` is a live reserved id. One atomic load when none is alive.
fn is_reserved(id: &Id) -> bool {
    if LIVE_RESERVED.load(Ordering::Acquire) == 0 {
        return false;
    }
    let raw = id.into_u64();
    if raw <= RESERVED_BASE || raw >= RESERVED_BASE + NEXT_SPAN.load(Ordering::Relaxed) {
        return false;
    }
    lock(&RESERVED)
        .as_ref()
        .is_some_and(|map| map.contains_key(&raw))
}

/// Count one more handle on reserved `id`.
fn clone_reserved(id: &Id) {
    if let Some(count) = lock(&RESERVED)
        .as_mut()
        .and_then(|map| map.get_mut(&id.into_u64()))
    {
        *count += 1;
    }
}

/// Drop one handle on reserved `id`; `true` when it was the last.
fn close_reserved(id: &Id) -> bool {
    let mut guard = lock(&RESERVED);
    let Some(map) = guard.as_mut() else {
        return false;
    };
    let raw = id.into_u64();
    let Some(count) = map.get_mut(&raw) else {
        return false;
    };
    *count -= 1;
    if *count > 0 {
        return false;
    }
    map.remove(&raw);
    LIVE_RESERVED.fetch_sub(1, Ordering::Release);
    true
}

// ── the forwarder ────────────────────────────────────────────────────────────

/// The global subscriber [`capture_boot_logs`] installs — see the module docs.
struct Forwarder;

impl Subscriber for Forwarder {
    fn on_register_dispatch(&self, subscriber: &Dispatch) {
        match TARGET.get() {
            Some(target) => target.on_register_dispatch(subscriber),
            None => {
                let _ = GLOBAL.set(subscriber.clone());
            }
        }
    }

    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        match TARGET.get() {
            Some(target) => target.register_callsite(metadata),
            None if DISABLED.load(Ordering::Relaxed) => Interest::never(),
            None => Interest::sometimes(),
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        match TARGET.get() {
            Some(target) => target.max_level_hint(),
            None if DISABLED.load(Ordering::Relaxed) => Some(LevelFilter::OFF),
            None => boot_filter().hint,
        }
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        match TARGET.get() {
            Some(target) => target.enabled(metadata),
            None if DISABLED.load(Ordering::Relaxed) => false,
            None => boot_filter().stack.enabled(metadata),
        }
    }

    fn event_enabled(&self, event: &Event<'_>) -> bool {
        match TARGET.get() {
            Some(target) => target.event_enabled(event),
            None => true,
        }
    }

    fn new_span(&self, span: &Attributes<'_>) -> Id {
        match TARGET.get() {
            Some(target) => match span.parent() {
                // The real registry has never seen that parent.
                Some(parent) if is_reserved(parent) => {
                    target.new_span(&Attributes::new(span.metadata(), span.values()))
                }
                _ => target.new_span(span),
            },
            None => reserve(),
        }
    }

    fn record(&self, span: &Id, values: &Record<'_>) {
        if let Some(target) = TARGET.get()
            && !is_reserved(span)
        {
            target.record(span, values);
        }
    }

    fn record_follows_from(&self, span: &Id, follows: &Id) {
        if let Some(target) = TARGET.get()
            && !is_reserved(span)
            && !is_reserved(follows)
        {
            target.record_follows_from(span, follows);
        }
    }

    fn event(&self, event: &Event<'_>) {
        if let Some(target) = TARGET.get() {
            return target.event(event);
        }
        let mut render = Render(Vec::new());
        event.record(&mut render);
        let mut buffer = lock(&BUFFER);
        // Re-checked under the lock: `take_over` switches while holding it.
        if let Some(target) = TARGET.get() {
            drop(buffer);
            return target.event(event);
        }
        if DISABLED.load(Ordering::Relaxed) {
            return;
        }
        if buffer.len() >= BUFFER_CAP {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        buffer.push(Stored {
            metadata: event.metadata(),
            fields: render.0,
        });
    }

    fn enter(&self, span: &Id) {
        if let Some(target) = TARGET.get()
            && !is_reserved(span)
        {
            target.enter(span);
        }
    }

    fn exit(&self, span: &Id) {
        if let Some(target) = TARGET.get()
            && !is_reserved(span)
        {
            target.exit(span);
        }
    }

    fn clone_span(&self, id: &Id) -> Id {
        if is_reserved(id) {
            clone_reserved(id);
            return id.clone();
        }
        match TARGET.get() {
            Some(target) => target.clone_span(id),
            None => id.clone(),
        }
    }

    fn try_close(&self, id: Id) -> bool {
        if is_reserved(&id) {
            return close_reserved(&id);
        }
        match TARGET.get() {
            Some(target) => target.try_close(id),
            None => false,
        }
    }

    fn current_span(&self) -> Current {
        match TARGET.get() {
            Some(target) => target.current_span(),
            // `Current::unknown()` is crate-private; the default
            // `Subscriber::current_span` returns it.
            None => tracing::subscriber::NoSubscriber::default().current_span(),
        }
    }

    unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
        if id == TypeId::of::<Self>() {
            return Some(self as *const Self as *const ());
        }
        // The target lives in a static for the rest of the process, so a
        // pointer into it stays valid. This is what keeps
        // `Dispatch::downcast_ref` (tracing-opentelemetry's `WithContext`)
        // reaching the real stack.
        // SAFETY: forwarding the caller's contract to the target.
        TARGET
            .get()
            .and_then(|target| unsafe { target.downcast_raw(id) })
    }
}

// ── public surface ───────────────────────────────────────────────────────────

/// Puts the forwarder in place as the global default, once per process.
/// `true` when it is (or already was) ours.
pub(crate) fn ensure_forwarder() -> bool {
    *OURS.get_or_init(|| {
        // Registering the forwarder computes every callsite's interest and
        // the max level from the boot filter: build it first.
        boot_filter();
        let ours = tracing::subscriber::set_global_default(Forwarder).is_ok();
        if ours {
            // What `try_init` would do: route `log` records into `tracing`.
            let _ = tracing_log::LogTracer::init();
        }
        ours
    })
}

/// Start capturing the logs emitted before [`install`](crate::install).
///
/// Installs the boot forwarder as the process-global `tracing` subscriber
/// (and the `log` → `tracing` bridge). Until `install` hands it the real
/// stack, events are buffered (at most 1024; one WARN reports how many were
/// dropped); `install` replays them in order through the installed filter,
/// then forwards everything.
///
/// What is buffered follows `RUST_LOG` as read by the first call (full
/// directive syntax, static directives only — span-scoped ones never match a
/// buffered event); with `RUST_LOG` unset or invalid, INFO and above.
/// `tracing.filter` is not loaded yet at this point, so it only applies at the
/// replay: a line it rejects is buffered but not printed.
///
/// When a subscriber was installed by someone else first, nothing is
/// captured: the returned handle is inert and `install` will report
/// [`AlreadyInstalled`](crate::AlreadyInstalled). Calling it again while still
/// buffering returns another live handle; [`install`](crate::install) calls it
/// itself when nobody did.
pub fn capture_boot_logs() -> BootLogs {
    BootLogs {
        live: ensure_forwarder(),
    }
}

/// Stop capturing: drop the buffered boot logs and make every callsite
/// disabled — what `tracing.enabled: false` means.
///
/// No effect once [`install`](crate::install) ran. A later `install` is still
/// allowed and turns logging on from then on (nothing buffered before is
/// replayed). Installs the forwarder if nobody did, so the disabled state is
/// the global one.
pub fn discard_boot_logs() {
    if !ensure_forwarder() {
        return;
    }
    {
        let mut buffer = lock(&BUFFER);
        if TARGET.get().is_some() {
            return;
        }
        DISABLED.store(true, Ordering::Relaxed);
        buffer.clear();
        DROPPED.store(0, Ordering::Relaxed);
    }
    tracing::callsite::rebuild_interest_cache();
}

/// The handle [`capture_boot_logs`] returns: held across the boot.
///
/// Dropping it while the forwarder is still buffering — the boot failed before
/// [`install`](crate::install) ran — writes the buffered lines to stderr in a
/// raw `LEVEL target: message k=v` form (see [`flush_boot_logs_to`]), so they
/// precede the boot error report instead of being lost. The forwarder keeps
/// buffering: a later capture or install proceeds normally. After the install
/// (or [`discard_boot_logs`]), or when the handle is inert, dropping it does
/// nothing.
#[must_use = "dropping the handle right away flushes the buffered boot logs to stderr"]
#[derive(Debug)]
pub struct BootLogs {
    live: bool,
}

impl Drop for BootLogs {
    fn drop(&mut self) {
        if self.live && TARGET.get().is_none() && !DISABLED.load(Ordering::Relaxed) {
            flush_boot_logs_to(&mut std::io::stderr());
        }
    }
}

/// Write every buffered boot log line to `out` as
/// `LEVEL target: message k=v k=v` and empty the buffer — what dropping a
/// live [`BootLogs`] does on stderr. Exposed for tests.
#[doc(hidden)]
pub fn flush_boot_logs_to(out: &mut dyn std::io::Write) {
    let stored = std::mem::take(&mut *lock(&BUFFER));
    let dropped = DROPPED.swap(0, Ordering::Relaxed);
    for event in &stored {
        let _ = writeln!(out, "{}", raw_line(event));
    }
    if dropped > 0 {
        let _ = writeln!(
            out,
            "WARN r2e_tracing: {dropped} boot log lines dropped before the subscriber was installed"
        );
    }
    let _ = out.flush();
}

fn raw_line(event: &Stored) -> String {
    let meta = event.metadata;
    let mut line = format!("{} {}:", meta.level(), meta.target());
    if let Some((_, message)) = event.fields.iter().find(|(f, _)| f.name() == "message") {
        let _ = write!(line, " {message}");
    }
    for (field, value) in event.fields.iter().filter(|(f, _)| f.name() != "message") {
        let _ = write!(line, " {}={value}", field.name());
    }
    line
}

/// Hand the forwarder the real stack: switch to Forwarding, rebuild the
/// interest cache, replay the buffer. `false` when a target was already set.
pub(crate) fn take_over(target: Arc<dyn Subscriber + Send + Sync>) -> bool {
    let stored = {
        let mut buffer = lock(&BUFFER);
        if TARGET.set(target).is_err() {
            return false;
        }
        std::mem::take(&mut *buffer)
    };
    let target = TARGET.get().expect("just set");
    if let Some(global) = GLOBAL.get() {
        target.on_register_dispatch(global);
    }
    // Interest and `LevelFilter::current()` were computed against the
    // buffering hints; recompute them against the real stack before anything
    // (the replay included) is filtered by them.
    tracing::callsite::rebuild_interest_cache();

    let dropped = DROPPED.swap(0, Ordering::Relaxed);
    let was_disabled = DISABLED.load(Ordering::Relaxed);
    for event in &stored {
        replay(target.as_ref(), event);
    }
    if dropped > 0 && !was_disabled {
        tracing::warn!(
            dropped,
            "boot log lines dropped before the subscriber was installed"
        );
    }
    true
}

/// Dispatch one stored event to `target`, as the macros would — `enabled`
/// and `event_enabled` included. Calling `target.event` directly bypasses the
/// callsite interest cache, so this check is the only thing applying the
/// installed filter to the replay: the boot filter may have kept a line the
/// installed one rejects (`RUST_LOG` invalid or changed, `tracing.filter`
/// stricter, a `Suppress` layer).
fn replay(target: &(dyn Subscriber + Send + Sync), event: &Stored) {
    let metadata = event.metadata;
    let fields = metadata.fields();
    let dispatch = |values: &tracing::field::ValueSet<'_>| {
        // A root event: its parent span (if any) was never seen by the target.
        let event = Event::new_child_of(None::<span::Id>, metadata, values);
        if target.enabled(metadata) && target.event_enabled(&event) {
            target.event(&event);
        }
    };
    let Some((first, _)) = event.fields.first() else {
        let none: [(&Field, Option<&dyn Value>); 0] = [];
        return dispatch(&fields.value_set(&none));
    };
    let mut values: [(&Field, Option<&dyn Value>); MAX_FIELDS] = [(first, None); MAX_FIELDS];
    for (slot, (field, value)) in values.iter_mut().zip(&event.fields) {
        *slot = (field, Some(value.as_value()));
    }
    dispatch(&fields.value_set(&values));
}
