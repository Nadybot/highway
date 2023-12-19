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
  "connections_per_ip": 2,
  "msg_freq_ratelimit": {
    "max_tokens": 10,
    "tokens": 10,
    "refill_amount": 10,
    "refill_millis": 1000
  },
  "msg_size_ratelimit": {
    "max_tokens": 5242880,
    "tokens": 5242880,
    "refill_amount": 524288,
    "refill_millis": 1000
  },
  "max_message_size": 1048576,
  "admin_password_hash": "$argon2id$v=19$m=4096,t=3,p=1$aGlnaHdheWlzc3VwcmVtZQ$KppnM084YRY1MkzMPteCzn+QF30mwFl9qIuwHUOsGfE",
  "public_channels": [
    {
      "name": "demo_room",
      "extra_info": { "hello": "world" },
      "msg_freq_ratelimit": {
        "max_tokens": 1,
        "tokens": 1,
        "refill_amount": 1,
        "refill_millis": 10000
      },
      "msg_size_ratelimit": {
        "max_tokens": 1048576,
        "tokens": 1048576,
        "refill_amount": 1024,
        "refill_millis": 10000
      }
    }
  ],
  "behind_proxy": false,
  "metrics_token": "supersecret"
}
```

- `port` sets the port to listen on (defaults to 3333)
- `connections_per_ip` set the maximum amount of connections one IP may open to the server (defaults to 50)
- `max_message_size` is a byte value and sets the limit for the websocket payloads (defaults to 1MB)
- `msg_freq_ratelimit` sets parameters for the ratelimiter of client messages (defaults to 10 per second)
- `msg_size_ratelimit` sets parameters for the ratelimit of client payload (defaults to 5MB per 10 seconds)
- `admin_password_hash` is an argon2id hash used for gaining admin access with the `Authorization` header. This can be generated with `echo -n "password" | argon2 myhash -id`
- `public_channels` is an array of special-named channels that can optionally be read only, i.e. only admins can send messages. These channels can optionally have channel-specific ratelimits
- `behind_proxy` will enable client IP parsing from the `X-Forwarded-For` header which is used in software like nginx
- `metrics_token` will set the token that has to be sent in the `Authorization` header for accessing the Prometheus metrics, which are exposed at `/metrics`

## Versioning

Highway's protocol respects semantic versioning. Highway server versions that are semver-compatible are semver-compatible on protocol level.

Do note that this means:

- Highway versions with a major version of `0` may have breaking changes in different minor releases
- Highway WILL NOT include new JSON fields in new patch releases
- Highway MIGHT include new JSON fields in new minor releases, no matter the major release

Highway sends its own version in the `X-Highway-Version` header to ALL HTTP requests made, even the websocket upgrade. Use this to verify that your client is compatible.

The following documentation applies to the current version of Highway only, not older ones.

## Connecting

Clients can connect at `ws://highway`. Optionally, the password from the specific argon2id hash can be provided to gain admin access via the `Authorization` HTTP header.

## Message format

Each payload needs to be valid JSON and contain a `type` key that is either `hello`, `join`, `leave`, `room_info`, `message`, `success` or `error`.
Read more about these types below.

## Hello

The `hello` type is received by the client when connecting and contains a field called `public_rooms` with a list of public rooms as well as a `config` field with the config parameters that are used for ratelimiting and client limitations. Ratelimits are leaky buckets with 4 self-explanatory parameters. Sizes for messages are in bytes.

Example:

```json
{
  "type": "hello",
  "public_rooms": ["boss_timers", "update_notifications"],
  "config": {
    "connections_per_ip": 50,
    "msg_freq_ratelimit": {
      "max_tokens": 100000,
      "tokens": 10,
      "refill_amount": 10,
      "refill_millis": 1000
    },
    "msg_size_ratelimit": {
      "max_tokens": 5242880,
      "tokens": 5242880,
      "refill_amount": 524288,
      "refill_millis": 1000
    },
    "max_message_size": 1048576
  }
}
```

## Join

The `join` type is received when a new user joins a room the client is connected to. It has a `user` field with a unique ID of the client that connected and a `room` field indicating the room that the client joined.

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

Optionally, an ID can be included with the join request under the `id` key and it will be included under the same key in the reply to the join request.

## Room-Info

The `room_info` type is received when joining a room and provides a list of connected clients, not including the client itself, as UUIDs, just like those you would receive when someone joins later. It also provides information as to whether a room is read-only or not.

Example:

```json
{
  "type": "room_info",
  "room": "boss_timers",
  "read_only": true,
  "users": ["6b8802d9-1bf8-4489-866b-9c197b8ebffc"]
}
```

Rooms may have arbitary JSON as additional metadata. It is sent with the `extra_info` JSON key inside `room_info`.

## Leave

The `leave` type is received when a user leaves a room the client is connected to. It has a `user` field with a unique ID of the client that left and a `room` field indicating the room that the client left.

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

Optionally, an ID can be included with the leave request under the `id` key and it will be included under the same key in the reply to the leave request.

## Message

Messages are the core of the server and require two more fields: `room` and `body`. `room` is the room the message will be sent to.

`body` can be any type of JSON (array, string, boolean, etc.) and should be where you place what should be relayed.

We recommend encrypting the body with a shared secret key, so noone can read what is being sent apart from intended recipients.

Example:

```json
{ "type": "message", "room": "boss_timers", "body": "hello world" }
```

When you receive a message, there will be an additional `user` field with the sender's UUID.

## Success and Error

`success` and `error` both have a `body` field that explains what was done/failed to be done. They contain useful information like invalid JSON, invalid rooms or room join confirmations.

If a room could not be joined or left, the `room` key will contain the name of the room that was attempted to join/leave.

If an `id` was specified in the request, the `success` or `error` reply will contain the same `id` as sent in the request.

For example, take this client request:

```json
{
  "type": "join",
  "room": "update_notifications",
  "id": "140fd478-47ef-4974-b0a8-a711e0b40767"
}
```

The server's reply could be:

```json
{
  "type": "success",
  "room": "update_notifications",
  "id": "140fd478-47ef-4974-b0a8-a711e0b40767",
  "body": "You joined the room"
}
```

but if the room doesn't exist, it could also be:

```json
{
  "type": "error",
  "room": "update_notifications",
  "id": "140fd478-47ef-4974-b0a8-a711e0b40767",
  "body": "The room name provided is shorter than 32 characters"
}
```
