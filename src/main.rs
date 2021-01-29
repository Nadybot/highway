use crate::json::from_slice;

use dashmap::DashSet;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    spawn,
    sync::{
        broadcast::{channel, Sender},
        mpsc::unbounded_channel,
    },
};
use tokio_tungstenite::{
    accept_async_with_config, connect_async,
    tungstenite::{Error, Message},
    WebSocketStream,
};

use std::{
    env::{set_var, var},
    io::Error as IoError,
    net::SocketAddr,
    sync::Arc,
};

mod constants;
mod json;
mod model;

type Broadcast = Sender<model::InternalMessage>;

async fn relay_from(source: &str, broadcast: Broadcast) -> Result<(), Error> {
    let (stream, _) = connect_async(source).await?;
    worker(stream, broadcast, true).await;
    Ok(())
}

async fn worker<S: 'static>(conn: WebSocketStream<S>, broadcast: Broadcast, relaying: bool)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let freq_ratelimiter = constants::get_freq_ratelimiter();
    let size_ratelimiter = constants::get_size_ratelimiter();
    let mut receiver = broadcast.subscribe();
    let rooms: Arc<DashSet<String>> = Arc::new(DashSet::new());

    let (mut write, mut read) = conn.split();
    let (tx, mut rx) = unbounded_channel();

    // Set of IDs that a relaying client is awaiting replies for
    let awaiting_reply = Arc::new(DashSet::new());

    let listen_rooms = rooms.clone();
    spawn(async move {
        while let Some(msg) = rx.recv().await {
            debug!("Sending websocket message: {:?}", msg);
            let _ = write.send(msg).await;
        }
    });

    let send_tx = tx.clone();
    let replies = awaiting_reply.clone();
    spawn(async move {
        while let Ok(msg) = receiver.recv().await {
            if let model::Payload::Message(m) = msg.payload {
                if (relaying && msg.was_relayed) || (!relaying && !listen_rooms.contains(&m.room)) {
                    continue;
                }
                if relaying {
                    replies.insert(m.id);
                }
            }
            let _ = send_tx.send(msg.tungstenite_message);
        }
    });

    while let Some(msg) = read.next().await {
        if let Ok(m) = msg {
            let amt = m.len();
            debug!("{:?}", m);
            match from_slice::<model::Payload>(&mut m.clone().into_data()) {
                Ok(payload) => match &payload {
                    model::Payload::Message(msg) => {
                        if !relaying {
                            let _ = freq_ratelimiter.acquire_one().await;
                            let _ = size_ratelimiter.acquire(amt).await;
                        } else {
                            let existed = awaiting_reply.remove(&msg.id).is_some();
                            if existed {
                                continue;
                            }
                        }

                        if rooms.contains(&msg.room) || relaying {
                            let msg = model::InternalMessage {
                                payload,
                                was_relayed: relaying,
                                tungstenite_message: m,
                            };
                            let _ = broadcast.send(msg);
                        } else {
                            let _ = tx.send(Message::Text(constants::INVALID_ROOM_MSG.to_string()));
                        }
                    }
                    model::Payload::Command(cmd) => match cmd {
                        model::Command::Join(j) => {
                            if constants::is_valid_room(&j.room) {
                                rooms.insert(j.room.clone());
                                let _ =
                                    tx.send(Message::text(constants::ROOM_JOIN_MSG.to_string()));
                            } else {
                                let _ =
                                    tx.send(Message::Text(constants::INVALID_ROOM_MSG.to_string()));
                            }
                        }
                        model::Command::Leave(l) => {
                            let was_in_room = rooms.remove(&l.room).is_some();
                            if was_in_room {
                                let _ =
                                    tx.send(Message::Text(constants::ROOM_LEAVE_MSG.to_string()));
                            } else {
                                let _ =
                                    tx.send(Message::Text(constants::INVALID_ROOM_MSG.to_string()));
                            }
                        }
                        model::Command::Hello(h) if relaying => {
                            for room in &h.public_rooms {
                                let msg = Message::Text(constants::get_join_payload(room));
                                let _ = tx.send(msg);
                            }
                        }
                        model::Command::NewPublicRoom(p) if relaying => {
                            constants::PUBLIC_CHANNELS.insert(p.room.clone());
                            let msg = Message::Text(constants::get_join_payload(&p.room));
                            let _ = tx.send(msg);
                            let msg = model::InternalMessage {
                                payload,
                                was_relayed: true,
                                tungstenite_message: m,
                            };
                            let _ = broadcast.send(msg);
                        }
                        _ => {
                            let _ = tx.send(Message::Text(constants::INVALID_CMD_MSG.to_string()));
                        }
                    },
                    // Ignore success and error messages
                    _ => {}
                },
                Err(e) => {
                    debug!("Error in JSON: {:?}", e);
                    let _ = tx.send(Message::Text(constants::INVALID_JSON_MSG.to_string()));
                }
            }
        }
    }

    info!("Connection to {:?} closed", rooms);
}

async fn handle_connection(
    raw_stream: TcpStream,
    addr: SocketAddr,
    broadcast: Broadcast,
) -> Result<(), Error> {
    info!("Incoming connection from {:?}", addr);
    let mut ws_stream = accept_async_with_config(raw_stream, Some(*constants::CONFIG)).await?;
    ws_stream
        .send(Message::Text(constants::get_hello_payload()))
        .await?;

    worker(ws_stream, broadcast, false).await;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), IoError> {
    if var("RUST_LOG").is_err() {
        set_var("RUST_LOG", "info");
    }
    env_logger::init();

    let port: u16 = var("PORT")
        .unwrap_or_else(|_| String::from("3333"))
        .parse()
        .unwrap();

    let addr: SocketAddr = ([0, 0, 0, 0], port).into();

    let (broadcast, _) = channel(1000);

    if let Ok(s) = var("RELAY_SOURCE") {
        let b = broadcast.clone();
        spawn(async move {
            if let Err(e) = relay_from(&s, b).await {
                error!("Relay failed: {}", e);
            };
        });
    }

    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    info!("Listening on: {}", addr);

    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(stream, addr, broadcast.clone()));
    }

    Ok(())
}
