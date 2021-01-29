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

Clients can connect at `ws://highway`.

## Message format

Each payload needs to be valid JSON and contain a `type` key that is either `command`, `message`, `success` or `error`.
Read more about these types below.

## Command

The command type requires an additional field called `cmd` with the value being `subscribe`, `unsubscribe`, `hello` or `new-public-room`. The latter two are read-only, i.e. only the server will send them.

`subscribe`, `unsubscribe` and `new-public-room` have a `room` field with the value being the room to join/leave/that was created.

Example:

```json
{
  "type": "command",
  "cmd": "subscribe",
  "room": "pvp"
}
```

would join the `pvp` room.

`hello` is sent when the client connects and has an array field `public-rooms` with a list of public rooms available.

## Message

Messages are the core of the server and require at _least_ two more fields: `id` and `room`. `room` is the room where the message will be sent to and `id` should be a unique identifier for the message (like a UUID).

Anything else is optional and is up to the client, for example `body` or `content`-like keys.

We recommend encrypting the body with a shared secret key, so noone can read what is being sent apart from intended recipients.

When a message is sent to the server, the client will get it echo'ed back. Ensuring you track the ID is crucial to avoid parsing it by accident.

## Success and Error

`success` and `error` both have a `message` field that explains what was done/failed to be done. They contain useful information like invalid JSON, invalid rooms or room join confirmations.
