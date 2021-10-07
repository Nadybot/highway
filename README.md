# highway

highway is a concurrent, multithreaded websocket server. It allows clients to connect to rooms, where messages are broadcasted to all other members of the room, very much like pub-sub.

## Running

From source:

```sh
cargo build --release
./target/release/highway
```

Alternatively, you can download a prebuilt binary from the CI by going [here](https://github.com/Nadybot/highway/actions), clicking on the first run with a green checkmark, scrolling down slightly to "Artifacts" and then selecting your operating system.

highway runs with sensible defaults, to override them, create a file called `config.json` with these contents:

```json
{
  "port": 3333,
  "connections_per_ip": 50,
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
- `connections_per_ip` set the maximum amount of connections one IP may open to the server (defaults to 50)
- `max_message_size` and `max_frame_size` are byte values and set limits for the websocket payloads (defaults to 1MB)
- `msg_per_sec` sets how many messages a client may send per second (defaults to 10)
- `bytes_per_10_sec` sets how many bytes a client may send per 10 seconds (defaults to 5MB)
- `admin_password_hash` is an argon2id hash used for gaining admin access with the `Authorization` header. This can be generated with `echo -n "password" | argon2 myhash -id`
- `public_channels` is an array of special-named channels that can optionally be read only, i.e. only admins can send messages

## Connecting

Clients can connect at `ws://highway`. Optionally, the password from the specific argon2id hash can be provided to gain admin access via the `Authorization` HTTP header.

## Message format

Each payload needs to be valid JSON and contain a `type` key that is either `hello`, `join`, `leave`, `room-info`, `message`, `success` or `error`.
Read more about these types below.

## Hello

The hello type is received by the client when connecting and contains a field called `public-rooms` with a list of public rooms as well as a `config` field with the config parameters that are used for ratelimiting.

Example:

```json
{
  "type": "hello",
  "public-rooms": ["boss_timers", "update_notifications"],
  "config": {
    "connections_per_ip": 50,
    "msg_per_sec": 10,
    "bytes_per_10_sec": 5242880,
    "max_message_size": 1048576,
    "max_frame_size": 1048576
  }
}
```

## Join

The join type is received when a new user joins a room the client is connected to. It has a `user` field with a unique ID of the client that connected and a `room` field indicating the room that the client joined.

```json
{
  "type": "join",
  "room": "boss_timers",
  "user": "80f627d1-aa78-4bf7-b9f3-7dfff1f57271"
}
```

It is also used to join a room with the same parameters, excluding `user`:

```json
{ "type": "join", "room": "boss_timers" }
```

## Room-Info

The room-info type is received when joining a room and provides a list of connected clients, not including the client itself, as UUIDs, just like those you would receive when someone joins later. It also provides information as to whether a room is read-only or not.

Example:

```json
{
  "type": "room-info",
  "room": "boss_timers",
  "read-only": true,
  "users": ["6b8802d9-1bf8-4489-866b-9c197b8ebffc"]
}
```

## Leave

The leave type is received when a user leaves a room the client is connected to. It has a `user` field with a unique ID of the client that left and a `room` field indicating the room that the client left.

```json
{
  "type": "leave",
  "room": "boss_timers",
  "user": "449c6639-0023-4262-a0b3-453e601d344a"
}
```

Just like join, leave can also be sent to the server to leave a room:

```json
{ "type": "leave", "room": "boss_timers" }
```

## Message

Messages are the core of the server and require two more fields: `room` and `body`. `room` is the room where the message will be sent to.

`body` can be any type of JSON (array, string, boolean, etc.) and should be where you place what should be relayed.

We recommend encrypting the body with a shared secret key, so noone can read what is being sent apart from intended recipients.

Example:

```json
{ "type": "message", "room": "boss_timers", "body": "hello world" }
```

When you receive a message, there will be an additional `user` field with the sender's UUID.

## Success and Error

`success` and `error` both have a `message` field that explains what was done/failed to be done. They contain useful information like invalid JSON, invalid rooms or room join confirmations.
