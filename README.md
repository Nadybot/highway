# highway

highway is a concurrent, multithreaded websocket server. It allows clients to connect to rooms, where messages are broadcasted to all other members of the room, very much like pub-sub.

## Running

```sh
cargo build --release
./target/release/highway
```

highway runs with sensible defaults, to override them, use enviroment variables:

- `PORT` sets the port to listen on (defaults to 3333)
- `MAX_MESSAGE_SIZE` and `MAX_FRAME_SIZE` are byte values and set limits for the websocket payloads (defaults to 1MB)
- `MSG_PER_SEC` sets how many messages a client may send per second (defaults to 10)
- `BYTES_PER_10_SEC` sets how many bytes a client may send per 10 seconds (defaults to 5MB)

## Connecting

Clients can connect to rooms via `ws://highway/stream?rooms=["room1", "room2"]`.

## Message format

Any message not following this format gets ignored.

```json
{
  "room": "room1",
  "body": "my encrypted text"
}
```

We recommend encrypting the body with a shared secret key, so noone can read what is being sent apart from intended recipients.
