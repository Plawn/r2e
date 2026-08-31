//! Handlers returning `impl Trait` compile under edition 2024 **without** a
//! hand-written `+ use<...>` clause.
//!
//! Every handler in this file would fail to compile if `#[routes]` re-emitted
//! it verbatim: under edition 2024 a return-position `impl Trait` captures the
//! `&self` lifetime, and the generated invocation moves the value out past that
//! borrow ("borrowed data escapes outside of method"). The clause `#[routes]`
//! appends is what keeps them compiling — see
//! `r2e-macros/src/codegen/precise_capture.rs`.
//!
//! The assertion is the compilation itself; the runtime test only proves the
//! routes are still wired.

use std::convert::Infallible;

use futures_core::Stream;
use r2e::http::response::{Sse, SseEvent, SseKeepAlive};
use r2e::prelude::*;
use r2e::web::sse::SseBroadcaster;

/// A plain injected bean the borrowing helper below can hand out a reference to.
#[derive(Clone)]
struct Label(String);

#[controller(path = "/capture")]
struct CaptureController {
    #[inject]
    bus: SseBroadcaster,
    #[inject]
    label: Label,
}

#[routes]
impl CaptureController {
    /// The canonical case: an `#[sse]` handler returning a bare stream.
    #[sse("/events")]
    async fn events(&self) -> impl Stream<Item = Result<SseEvent, Infallible>> {
        self.bus.subscribe()
    }

    /// A plain route returning the response type with the stream nested one
    /// generic argument deep — the clause has to land on the INNER `impl
    /// Trait`, not on the `Sse<_>` wrapper.
    #[get("/wrapped")]
    async fn wrapped(&self) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
        Sse::new(self.bus.subscribe()).keep_alive(SseKeepAlive::default())
    }

    /// A route returning a bare `impl IntoResponse`, using the helper below —
    /// so the body genuinely reads `&self`.
    #[get("/text")]
    async fn text(&self) -> impl IntoResponse {
        self.tag().to_string()
    }

    /// An explicit clause is preserved, not duplicated: the `use<>` written by
    /// hand must still be the only one on the signature.
    #[sse("/explicit")]
    async fn explicit(&self) -> impl Stream<Item = Result<SseEvent, Infallible>> + use<> {
        self.bus.subscribe()
    }

    /// A `#[request_helper]` returning a value that borrows `&self` keeps
    /// working: the rewrite applies to handlers only, never to helpers, where
    /// borrowing from the receiver is legitimate. Adding `use<>` here would
    /// turn this into a compile error.
    #[request_helper]
    fn tag(&self) -> impl std::fmt::Display + '_ {
        &self.label.0
    }
}

#[tokio::test]
async fn handlers_are_wired() {
    let router = AppBuilder::new()
        .provide(SseBroadcaster::new(8))
        .provide(Label("capture".to_string()))
        .build_state()
        .await
        .register_controller::<CaptureController>()
        .build();
    // Building the router is the wiring assertion; the compile of the impl
    // block above is the actual subject of this file.
    let _ = router;
}
