# highway

highway is a concurrent, multithreaded websocket server. It allows clients to connect to rooms, where messages are broadcasted to all other members of the room, very much like pub-sub.

## Running

```sh
cargo build --release
./target/release/highway
```

highway runs with sensible defaults, to override them, create a file called `config.json` with these contents:

```json
{
  "port": 3333,
  "msg_per_sec": 10,
  "bytes_per_10_sec": 5242880,
  "max_message_size": 1048576,
  "max_frame_size": 1048576,
  "admin_password_hash": "$argon2id$v=19$m=4096,t=3,p=1$aGlnaHdheWlzc3VwcmVtZQ$KppnM084YRY1MkzMPteCzn+QF30mwFl9qIuwHUOsGfE",
  "public_channels": [
    {
      "name": "boss_timers",
      "read_only": true
    },
    {
      "name": "update_notifications"
    }
  ]
}
```

- `port` sets the port to listen on (defaults to 3333)
- `max_message_size` and `max_frame_size` are byte values and set limits for the websocket payloads (defaults to 1MB)
- `msg_per_sec` sets how many messages a client may send per second (defaults to 10)
- `bytes_per_10_sec` sets how many bytes a client may send per 10 seconds (defaults to 5MB)
- `admin_password_hash` is an argon2id hash used for gaining admin access with the `Authorization` header. This can be generated with `echo -n "password" | argon2 myhash -id`
- `public_channels` is an array of special-named channels that can optionally be read only, i.e. only admins can send messages

## Connecting

Clients can connect at `ws://highway`. Optionally, the password from the specific argon2id hash can be provided to gain admin access via the `Authorization` HTTP header.

## Message format

Each payload needs to be valid JSON and contain a `type` key that is either `command`, `message`, `success` or `error`.
Read more about these types below.

## Command

The command type requires an additional field called `cmd` with the value being `subscribe` or `unsubscribe`.

It also requires a `room` field with the value being the room to join or leave.

Example:

```json
{
  "type": "command",
  "cmd": "subscribe",
  "room": "dfcdde5f-e781-49b0-bbfa-e5ee1568c83a"
}
```

would join the `dfcdde5f-e781-49b0-bbfa-e5ee1568c83a` room.

## Message

Messages are the core of the server and require at one more field: `room`. `room` is the room where the message will be sent to.

Anything else is optional and is up to the client, for example `body` or `content`-like keys.

We recommend encrypting the body with a shared secret key, so noone can read what is being sent apart from intended recipients.

When a message is sent to the server, the client will get it echo'ed back. Ensuring you track the ID is crucial to avoid parsing it by accident.

## Success and Error

`success` and `error` both have a `message` field that explains what was done/failed to be done. They contain useful information like invalid JSON, invalid rooms or room join confirmations.
