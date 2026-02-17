# example-websocket-chat

Real-time chat with named rooms, demonstrating:

- `WsRooms` for named chat rooms
- `send_json_from(client_id, ...)` for sender exclusion
- JSON-structured WebSocket protocol (`#[serde(tag = "type")]`)
- `EventBus::emit()` fire-and-forget for message persistence
- `#[consumer(bus = "event_bus")]` declarative event handling
- SQLite for message history
- REST endpoints for room listing and history

## Running

```bash
cargo run -p example-websocket-chat
```

## Endpoints

| Protocol | Path | Description |
|----------|------|-------------|
| WS | `/chat/{room}?username=Alice` | Join a chat room |
| GET | `/rooms` | List active rooms |
| GET | `/rooms/{room}/history` | Get message history |
| GET | `/health` | Health check |

## WebSocket protocol

Connect: `ws://localhost:3000/chat/general?username=Alice`

Send (JSON):
```json
{"type": "message", "text": "Hello!"}
```

Receive (JSON):
```json
{"type": "message", "username": "Alice", "text": "Hello!", "room": "general"}
{"type": "join", "username": "Bob", "room": "general"}
{"type": "leave", "username": "Bob", "room": "general"}
```

## Test with websocat

```bash
# Terminal 1
websocat 'ws://localhost:3000/chat/general?username=Alice'

# Terminal 2
websocat 'ws://localhost:3000/chat/general?username=Bob'
```
