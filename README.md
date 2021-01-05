# highway

highway is a concurrent, multithreaded websocket server. It allows clients to connect to rooms, where messages are broadcasted to all other members of the room, very much like pub-sub.

## Running

```sh
cargo build --release
PORT=3333 ./target/release/highway
```

## Connecting

Clients can connect to `ws://highway/room-name`. We recommend encrypting the messages sent there with a shared secret key, so noone can read what is being sent apart from intended recipients.
