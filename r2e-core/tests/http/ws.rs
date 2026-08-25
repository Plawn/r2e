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
    use r2e_core::http::Router;
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
        let router = Router::new().route(
            "/ws",
            r2e_core::http::routing::get(move |upgrade: WebSocketUpgrade| {
                let f = f.clone();
                let tx = tx.clone();
                async move {
                    upgrade.on_upgrade(move |socket| async move {
                        let result = f(WsStream::new(socket)).await;
                        if let Some(tx) = tx.lock().unwrap().take() {
                            let _ = tx.send(result);
                        }
                    })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        r2e_core::rt::spawn(async move {
            r2e_core::http::serve(listener, router).await.unwrap();
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
