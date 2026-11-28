use r2e_core::http::ws::Message;
use r2e_core::web::ws::{WsBroadcaster, WsRooms};

#[r2e_core::test]
async fn broadcaster_send_recv() {
    let broadcaster = WsBroadcaster::new(16);
    let mut rx = broadcaster.subscribe();
    broadcaster.send_text("hello");
    let msg = rx.recv().await.unwrap();
    match &*msg {
        Message::Text(t) => assert_eq!(t.to_string(), "hello"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[r2e_core::test]
async fn broadcaster_excludes_sender() {
    let broadcaster = WsBroadcaster::new(16);
    let mut rx = broadcaster.subscribe();
    let sender_id = rx.client_id();

    // Send from the same client id — should be skipped
    broadcaster.send_text_from(sender_id, "self-msg");

    // Send from a different client id — should be received
    broadcaster.send_text_from(sender_id + 999, "other-msg");

    let msg = rx.recv().await.unwrap();
    match &*msg {
        Message::Text(t) => assert_eq!(t.to_string(), "other-msg"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[r2e_core::test]
async fn rooms_get_or_create() {
    let rooms: WsRooms = WsRooms::new(16);
    let _b1 = rooms.room("chat".to_string());
    let _b2 = rooms.room("chat".to_string());
    // Both should work (same room reused internally)
    assert_eq!(rooms.room_count(), 1);
}

#[r2e_core::test]
async fn rooms_remove() {
    let rooms: WsRooms = WsRooms::new(16);
    let _b = rooms.room("chat".to_string());
    assert_eq!(rooms.room_count(), 1);
    rooms.remove("chat");
    assert_eq!(rooms.room_count(), 0);
    // Creating again should work
    let _b2 = rooms.room("chat".to_string());
    assert_eq!(rooms.room_count(), 1);
}

#[r2e_core::test]
async fn rooms_reap_empty_drops_subscriberless_rooms() {
    let rooms: WsRooms = WsRooms::new(16);
    // Room with a live subscriber (kept alive across the reap).
    let kept_broadcaster = rooms.room("kept".to_string());
    let _rx = kept_broadcaster.subscribe();
    // Room with no subscriber (will be reaped).
    let _abandoned = rooms.room("abandoned".to_string());

    assert_eq!(rooms.room_count(), 2);
    let reaped = rooms.reap_empty();
    assert_eq!(reaped, 1);
    assert_eq!(rooms.room_count(), 1);
}

#[r2e_core::test]
async fn rooms_typed_key() {
    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    struct ChatRoomId(u64);

    let rooms: WsRooms<ChatRoomId> = WsRooms::new(8);
    let _b = rooms.room(ChatRoomId(1));
    assert_eq!(rooms.room_count(), 1);
    rooms.remove(&ChatRoomId(1));
    assert_eq!(rooms.room_count(), 0);
}

// ── WsStream batching: feed / flush / close over a live socket ───────────

mod batching {
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use r2e_core::http::ws::{Message, WebSocketUpgrade};
    use r2e_core::http::Routes;
    use r2e_core::web::ws::{WsError, WsStream};
    use tokio_tungstenite::tungstenite::Message as ClientMessage;

    type ClientStream = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    /// Serve one `/ws` route whose handler drives `WsStream` through `f`, and
    /// return a connected client. Server errors surface via `tx`.
    async fn connect<F, Fut>(
        f: F,
    ) -> (
        ClientStream,
        tokio::sync::oneshot::Receiver<Result<(), WsError>>,
    )
    where
        F: Fn(WsStream) -> Fut + Clone + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), WsError>> + Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        let router = Routes::new().route(
            "/ws",
            r2e_core::http::routing::get(move |_: (), req: r2e_core::http::Request| {
                let f = f.clone();
                let tx = tx.clone();
                async move {
                    use r2e_core::http::extract::FromRequestParts;
                    let (mut parts, _) = req.into_parts();
                    let upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &())
                        .await
                        .map_err(r2e_core::http::response::IntoResponse::into_response)?;
                    Ok::<_, r2e_core::http::Response>(upgrade.on_upgrade(
                        move |socket| async move {
                            let result = f(WsStream::new(socket)).await;
                            if let Some(tx) = tx.lock().unwrap().take() {
                                let _ = tx.send(result);
                            }
                        },
                    ))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        r2e_core::rt::spawn(async move {
            r2e_core::http::server::serve(
                listener,
                router,
                std::future::pending::<()>(),
                Default::default(),
            )
            .await
            .unwrap();
        });
        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
            .await
            .expect("client connect");
        (client, rx)
    }

    async fn recv(client: &mut ClientStream) -> ClientMessage {
        tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("timed out waiting for a frame")
            .expect("stream ended")
            .expect("protocol error")
    }

    async fn recv_text(client: &mut ClientStream) -> String {
        match recv(client).await {
            ClientMessage::Text(t) => t.to_string(),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    async fn recv_binary(client: &mut ClientStream) -> Vec<u8> {
        match recv(client).await {
            ClientMessage::Binary(b) => b.to_vec(),
            other => panic!("expected Binary, got {other:?}"),
        }
    }

    /// `true` when nothing arrives within `dur` (negative assertion).
    async fn silent_for(client: &mut ClientStream, dur: Duration) -> bool {
        tokio::time::timeout(dur, client.next()).await.is_err()
    }

    #[r2e_core::test]
    async fn feed_then_flush_preserves_order() {
        let (mut client, done) = connect(|mut ws| async move {
            for i in 0..8 {
                ws.feed_text(format!("frame-{i}")).await?;
            }
            ws.flush().await?;
            // Keep the connection open until the client has read everything.
            let _ = ws.next().await;
            Ok(())
        })
        .await;

        for i in 0..8 {
            assert_eq!(recv_text(&mut client).await, format!("frame-{i}"));
        }
        client.close(None).await.unwrap();
        assert!(done.await.unwrap().is_ok());
    }

    #[r2e_core::test]
    async fn feed_does_not_write_until_flush() {
        let (mut client, _done) = connect(|mut ws| async move {
            ws.feed_text("queued").await?;
            // Hold the frame in the sink; only flush once the client asks.
            let _ = ws.next().await;
            ws.flush().await?;
            let _ = ws.next().await;
            Ok(())
        })
        .await;

        assert!(
            silent_for(&mut client, Duration::from_millis(200)).await,
            "a fed frame must not reach the wire before flush()"
        );
        client.send(ClientMessage::Text("go".into())).await.unwrap();
        assert_eq!(recv_text(&mut client).await, "queued");
        client.close(None).await.unwrap();
    }

    #[r2e_core::test]
    async fn feed_binary_keeps_bytes_intact_across_16_frames_256kib() {
        // Acceptance criterion: 16 frames / 256 KiB, one flush, payload unchanged.
        const FRAMES: usize = 16;
        const FRAME_LEN: usize = 16 * 1024;
        let payloads: Vec<bytes::Bytes> = (0..FRAMES)
            .map(|i| {
                let mut v = vec![0u8; FRAME_LEN];
                for (j, b) in v.iter_mut().enumerate() {
                    *b = ((i * 31 + j * 7) % 256) as u8;
                }
                bytes::Bytes::from(v)
            })
            .collect();
        let expected = payloads.clone();

        let (mut client, done) = connect(move |mut ws| {
            let payloads = payloads.clone();
            async move {
                for p in payloads {
                    ws.feed_binary(p).await?;
                }
                ws.flush().await?;
                let _ = ws.next().await;
                Ok(())
            }
        })
        .await;

        for want in &expected {
            let got = recv_binary(&mut client).await;
            assert_eq!(got.len(), FRAME_LEN);
            assert_eq!(&got[..], &want[..]);
        }
        client.close(None).await.unwrap();
        assert!(done.await.unwrap().is_ok());
    }

    #[r2e_core::test]
    async fn send_is_immediately_visible() {
        let (mut client, _done) = connect(|mut ws| async move {
            ws.send_text("now").await?;
            let _ = ws.next().await;
            Ok(())
        })
        .await;
        assert_eq!(recv_text(&mut client).await, "now");
        client.close(None).await.unwrap();
    }

    #[r2e_core::test]
    async fn send_after_feed_flushes_the_queue_in_order() {
        let (mut client, _done) = connect(|mut ws| async move {
            ws.feed_text("a").await?;
            ws.feed_text("b").await?;
            ws.send_text("c").await?; // flushes a, b, c
            let _ = ws.next().await;
            Ok(())
        })
        .await;
        assert_eq!(recv_text(&mut client).await, "a");
        assert_eq!(recv_text(&mut client).await, "b");
        assert_eq!(recv_text(&mut client).await, "c");
        client.close(None).await.unwrap();
    }

    #[r2e_core::test]
    async fn flush_with_nothing_queued_is_ok() {
        let (mut client, done) = connect(|mut ws| async move {
            ws.flush().await?;
            ws.flush().await?;
            ws.send_text("after-empty-flush").await?;
            let _ = ws.next().await;
            Ok(())
        })
        .await;
        assert_eq!(recv_text(&mut client).await, "after-empty-flush");
        client.close(None).await.unwrap();
        assert!(done.await.unwrap().is_ok());
    }

    #[r2e_core::test]
    async fn close_flushes_pending_frames_then_closes() {
        let (mut client, done) = connect(|mut ws| async move {
            ws.feed_text("last-words").await?;
            ws.close().await?;
            Ok(())
        })
        .await;
        assert_eq!(recv_text(&mut client).await, "last-words");
        assert!(
            matches!(recv(&mut client).await, ClientMessage::Close(_)),
            "close() must send the close handshake after the queued frame"
        );
        assert!(done.await.unwrap().is_ok());
    }

    #[r2e_core::test]
    async fn feed_after_close_is_an_error() {
        let (_client, done) = connect(|mut ws| async move {
            ws.close().await?;
            let after = ws.feed_text("too late").await;
            let flushed = ws.flush().await;
            match (after, flushed) {
                (Err(WsError::Send(_)), _) | (Ok(()), Err(WsError::Send(_))) => Ok(()),
                other => panic!("expected a Send error after close, got {other:?}"),
            }
        })
        .await;
        assert!(done.await.unwrap().is_ok());
    }

    #[r2e_core::test]
    async fn send_error_after_peer_disconnect_is_propagated() {
        let (client, done) = connect(|mut ws| async move {
            // Wait for the peer to go away, then try to write.
            while ws.next().await.is_some() {}
            let mut last = Ok(());
            for _ in 0..8 {
                last = ws.send_text("into the void").await;
                if last.is_err() {
                    break;
                }
            }
            match last {
                Err(WsError::Send(_)) => Ok(()),
                Ok(()) => panic!("writes to a dead peer never failed"),
                Err(other) => panic!("unexpected error variant: {other:?}"),
            }
        })
        .await;
        drop(client);
        assert!(done.await.unwrap().is_ok());
    }

    #[r2e_core::test]
    async fn sink_impl_send_all_delivers_in_order() {
        let (mut client, done) = connect(|mut ws| async move {
            let items = (0..4).map(|i| Ok::<_, WsError>(Message::Text(format!("s{i}").into())));
            let mut stream = futures_util::stream::iter(items);
            SinkExt::send_all(&mut ws, &mut stream).await?;
            let _ = ws.next().await;
            Ok(())
        })
        .await;
        for i in 0..4 {
            assert_eq!(recv_text(&mut client).await, format!("s{i}"));
        }
        client.close(None).await.unwrap();
        assert!(done.await.unwrap().is_ok());
    }

    #[r2e_core::test]
    async fn feed_honours_backpressure_and_frames_survive_a_slow_reader() {
        // A reader that does not drain for a while: the server keeps feeding
        // and flushing large frames. `feed` must await readiness instead of
        // failing, and every byte must still arrive in order once the reader
        // catches up.
        const FRAMES: usize = 64;
        const FRAME_LEN: usize = 64 * 1024; // 4 MiB total, well past socket buffers
        let (mut client, done) = connect(|mut ws| async move {
            for i in 0..FRAMES {
                let payload = bytes::Bytes::from(vec![i as u8; FRAME_LEN]);
                ws.feed_binary(payload).await?;
                if i % 8 == 7 {
                    ws.flush().await?;
                }
            }
            ws.flush().await?;
            let _ = ws.next().await;
            Ok(())
        })
        .await;

        // Stall the reader so the kernel buffers fill and the sink pushes back.
        tokio::time::sleep(Duration::from_millis(300)).await;

        for i in 0..FRAMES {
            let got = recv_binary(&mut client).await;
            assert_eq!(got.len(), FRAME_LEN, "frame {i} length");
            assert!(got.iter().all(|&b| b == i as u8), "frame {i} content");
        }
        client.close(None).await.unwrap();
        assert!(done.await.unwrap().is_ok());
    }
}

// ── WsStream closing handshake: close / close_with / 1001 / 1009 ─────────
//
// The server side of these tests runs WITHOUT `TCP_NODELAY` (the bare `serve`
// default), which is what makes a missing handshake visible on a loopback:
// with it set, the close frame usually reaches the client before the reset
// does. Both ends write while the server closes — the condition under which a
// socket dropped with unread inbound frames is reset by the kernel.

mod closing {
    use std::time::{Duration, Instant};

    use futures_util::{SinkExt, StreamExt};
    use r2e_core::http::ws::{Message, WebSocket, WebSocketUpgrade};
    use r2e_core::http::Routes;
    use r2e_core::rt::CancelToken;
    use r2e_core::web::ws::{
        WsError, WsStream, CLOSE_GOING_AWAY, CLOSE_HANDSHAKE_TIMEOUT, CLOSE_MESSAGE_TOO_BIG,
        SHUTDOWN_CLOSE_HANDSHAKE_TIMEOUT,
    };
    use tokio_tungstenite::tungstenite::Message as ClientMessage;

    type ClientStream = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    /// How the client learned the session was over.
    #[derive(Debug, PartialEq, Eq)]
    enum Ending {
        /// A close frame, with its code and reason (`None` when it carried none).
        Closed(Option<(u16, String)>),
        /// The stream ended, or failed, before any close frame.
        Lost(String),
    }

    /// Serve one `/ws` route — `limit` is the route's `max_message_size`,
    /// `build` turns the raw socket into the `WsStream` under test — and
    /// return a connected client plus the handler's result.
    async fn connect<B, F, Fut>(
        limit: Option<usize>,
        build: B,
        f: F,
    ) -> (
        ClientStream,
        tokio::sync::oneshot::Receiver<Result<(), WsError>>,
    )
    where
        B: Fn(WebSocket) -> WsStream + Clone + Send + Sync + 'static,
        F: Fn(WsStream) -> Fut + Clone + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), WsError>> + Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        let router = Routes::new().route(
            "/ws",
            r2e_core::http::routing::get(move |_: (), req: r2e_core::http::Request| {
                let f = f.clone();
                let build = build.clone();
                let tx = tx.clone();
                async move {
                    use r2e_core::http::extract::FromRequestParts;
                    let (mut parts, _) = req.into_parts();
                    let upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &())
                        .await
                        .map_err(r2e_core::http::response::IntoResponse::into_response)?;
                    let upgrade = r2e_core::web::ws::__limit_upgrade(upgrade, limit, None);
                    Ok::<_, r2e_core::http::Response>(upgrade.on_upgrade(
                        move |socket| async move {
                            let result = f(build(socket)).await;
                            if let Some(tx) = tx.lock().unwrap().take() {
                                let _ = tx.send(result);
                            }
                        },
                    ))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        r2e_core::rt::spawn(async move {
            r2e_core::http::server::serve(
                listener,
                router,
                std::future::pending::<()>(),
                Default::default(),
            )
            .await
            .unwrap();
        });
        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
            .await
            .expect("client connect");
        (client, rx)
    }

    /// Drive a client that writes one frame per millisecond for as long as the
    /// socket accepts them, and report how the session ended on its read side.
    async fn chatter_until_the_end(client: ClientStream) -> Ending {
        let (mut sink, mut stream) = client.split();
        let writer = r2e_core::rt::spawn(async move {
            while sink.send(ClientMessage::text("client frame")).await.is_ok() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        let ending = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match stream.next().await {
                    Some(Ok(ClientMessage::Close(frame))) => {
                        return Ending::Closed(
                            frame.map(|f| (u16::from(f.code), f.reason.to_string())),
                        );
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Ending::Lost(e.to_string()),
                    None => return Ending::Lost("stream ended".into()),
                }
            }
        })
        .await
        .expect("the session never ended");
        writer.abort();
        ending
    }

    /// The handler's result. A handler that panicked never reports one, so
    /// this is bounded: the test fails instead of hanging.
    async fn finished(
        done: tokio::sync::oneshot::Receiver<Result<(), WsError>>,
    ) -> Result<(), WsError> {
        tokio::time::timeout(Duration::from_secs(10), done)
            .await
            .expect("the handler never finished (it panicked or hung)")
            .expect("the handler was dropped before finishing")
    }

    /// The server writes without reading, so the client's frames pile up
    /// unread, then ends the session through `end`.
    async fn write_then<E, Fut>(mut ws: WsStream, end: E) -> Result<(), WsError>
    where
        E: FnOnce(WsStream) -> Fut,
        Fut: std::future::Future<Output = Result<(), WsError>>,
    {
        // Let the client's frames start piling up unread, then write a burst
        // right before closing: without `TCP_NODELAY` the close frame queues
        // behind those unacknowledged segments, and a socket dropped at that
        // point discards it along with the reset.
        tokio::time::sleep(Duration::from_millis(20)).await;
        for i in 0..20 {
            ws.send_text(format!("server frame {i}")).await?;
        }
        end(ws).await
    }

    const ROUNDS: usize = 30;

    #[r2e_core::test]
    async fn close_with_reaches_a_peer_that_is_still_writing() {
        for round in 0..ROUNDS {
            let (client, done) = connect(None, WsStream::new, |ws| {
                write_then(ws, |mut ws| async move {
                    ws.close_with(4001, "ticket expired").await
                })
            })
            .await;
            assert_eq!(
                chatter_until_the_end(client).await,
                Ending::Closed(Some((4001, "ticket expired".into()))),
                "round {round}: the close frame must survive a writing peer"
            );
            assert!(finished(done).await.is_ok());
        }
    }

    #[r2e_core::test]
    async fn close_reaches_a_peer_that_is_still_writing() {
        for round in 0..ROUNDS {
            let (client, done) = connect(None, WsStream::new, |ws| {
                write_then(ws, |mut ws| async move { ws.close().await })
            })
            .await;
            assert_eq!(
                chatter_until_the_end(client).await,
                Ending::Closed(None),
                "round {round}: the close frame must survive a writing peer"
            );
            assert!(finished(done).await.is_ok());
        }
    }

    #[r2e_core::test]
    async fn going_away_reaches_a_peer_that_is_still_writing() {
        for round in 0..ROUNDS {
            let token = CancelToken::new();
            let fired = token.clone();
            let (client, done) = connect(
                None,
                move |socket| WsStream::with_shutdown(socket, Some(token.clone())),
                move |ws| {
                    let fired = fired.clone();
                    write_then(ws, |mut ws| async move {
                        fired.cancel();
                        assert!(ws.next().await.is_none(), "shutdown ends the stream");
                        assert!(ws.next().await.is_none(), "and it stays ended");
                        Ok(())
                    })
                },
            )
            .await;
            assert_eq!(
                chatter_until_the_end(client).await,
                Ending::Closed(Some((CLOSE_GOING_AWAY, "server shutting down".into()))),
                "round {round}: the 1001 must survive a writing peer"
            );
            assert!(finished(done).await.is_ok());
        }
    }

    #[r2e_core::test]
    async fn close_returns_as_soon_as_the_peer_answers() {
        let (mut client, done) = connect(None, WsStream::new, |mut ws| async move {
            let started = Instant::now();
            ws.close_with(1000, "done").await?;
            assert!(
                started.elapsed() < CLOSE_HANDSHAKE_TIMEOUT / 2,
                "an answering peer must end the wait, took {:?}",
                started.elapsed()
            );
            assert!(
                ws.next().await.is_none(),
                "a closed stream reports end-of-stream"
            );
            Ok(())
        })
        .await;
        // Reading the close frame queues the client's answer; the next poll
        // flushes it.
        while let Some(Ok(_)) = client.next().await {}
        assert!(finished(done).await.is_ok());
    }

    #[r2e_core::test]
    async fn close_gives_up_on_a_silent_peer_after_the_handshake_timeout() {
        // The client never reads, so it never answers the close frame.
        let (_client, done) = connect(None, WsStream::new, |mut ws| async move {
            let started = Instant::now();
            ws.close().await?;
            let waited = started.elapsed();
            assert!(
                waited >= CLOSE_HANDSHAKE_TIMEOUT && waited < CLOSE_HANDSHAKE_TIMEOUT * 3,
                "expected about {CLOSE_HANDSHAKE_TIMEOUT:?}, waited {waited:?}"
            );
            Ok(())
        })
        .await;
        assert!(finished(done).await.is_ok());
    }

    #[r2e_core::test]
    async fn going_away_gives_up_on_a_silent_peer_after_the_shutdown_timeout() {
        let token = CancelToken::new();
        token.cancel();
        let (_client, done) = connect(
            None,
            move |socket| WsStream::with_shutdown(socket, Some(token.clone())),
            |mut ws| async move {
                let started = Instant::now();
                assert!(ws.next().await.is_none());
                let waited = started.elapsed();
                assert!(
                    waited >= SHUTDOWN_CLOSE_HANDSHAKE_TIMEOUT && waited < CLOSE_HANDSHAKE_TIMEOUT,
                    "expected about {SHUTDOWN_CLOSE_HANDSHAKE_TIMEOUT:?}, waited {waited:?}"
                );
                Ok(())
            },
        )
        .await;
        assert!(finished(done).await.is_ok());
    }

    #[r2e_core::test]
    async fn oversized_message_is_refused_with_1009() {
        let (mut client, done) = connect(Some(1024), WsStream::new, |mut ws| async move {
            match ws.next().await {
                Some(Err(WsError::Recv(_))) => {}
                other => panic!("expected a receive error, got {other:?}"),
            }
            assert!(
                ws.next().await.is_none(),
                "the stream is over after the refusal"
            );
            Ok(())
        })
        .await;
        client
            .send(ClientMessage::binary(vec![7u8; 4096]))
            .await
            .unwrap();
        let frame = loop {
            match tokio::time::timeout(Duration::from_secs(5), client.next())
                .await
                .expect("timed out waiting for the close frame")
            {
                Some(Ok(ClientMessage::Close(frame))) => break frame.expect("a coded close frame"),
                Some(Ok(_)) => {}
                other => panic!("expected a 1009 close frame, got {other:?}"),
            }
        };
        assert_eq!(u16::from(frame.code), CLOSE_MESSAGE_TOO_BIG);
        assert_eq!(frame.reason.to_string(), "message too big");
        assert!(finished(done).await.is_ok());
    }

    #[r2e_core::test]
    async fn message_within_the_limit_is_not_refused() {
        let (mut client, done) = connect(Some(1024), WsStream::new, |mut ws| async move {
            match ws.next().await {
                Some(Ok(Message::Binary(bytes))) => assert_eq!(bytes.len(), 1024),
                other => panic!("expected the binary message, got {other:?}"),
            }
            Ok(())
        })
        .await;
        client
            .send(ClientMessage::binary(vec![7u8; 1024]))
            .await
            .unwrap();
        assert!(finished(done).await.is_ok());
    }
}

// ─── requested_protocols ───

fn offered(lines: &[&str]) -> Vec<String> {
    let mut headers = r2e_core::http::HeaderMap::new();
    for line in lines {
        headers.append(
            r2e_core::http::header::SEC_WEBSOCKET_PROTOCOL,
            line.parse().unwrap(),
        );
    }
    r2e_core::web::ws::requested_protocols(&headers)
        .map(str::to_string)
        .collect()
}

#[test]
fn requested_protocols_splits_and_trims_the_offer() {
    assert_eq!(
        offered(&["collab.v1, app.ticket.abc ,collab.v2"]),
        ["collab.v1", "app.ticket.abc", "collab.v2"]
    );
}

#[test]
fn requested_protocols_reads_every_header_line_in_order() {
    assert_eq!(offered(&["a, b", "c"]), ["a", "b", "c"]);
}

#[test]
fn requested_protocols_is_empty_without_the_header() {
    assert!(offered(&[]).is_empty());
    assert!(offered(&[" , "]).is_empty());
}
