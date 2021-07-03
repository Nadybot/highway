use crate::{config::CONFIG, json::from_slice};

use argon2::{
    password_hash::{PasswordHash, PasswordVerifier},
    Argon2,
};
use dashmap::{DashMap, DashSet};
use futures_util::{SinkExt, StreamExt};
use log::{debug, info};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    spawn,
    sync::mpsc::{unbounded_channel, UnboundedSender},
};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        handshake::server::{Request, Response},
        protocol::WebSocketConfig,
        Error, Message,
    },
    WebSocketStream,
};

use std::{
    env::{set_var, var},
    io::Error as IoError,
    net::SocketAddr,
    sync::Arc,
};

mod config;
mod constants;
mod json;
mod model;

// Name: (read_only, send_handles)
type State = DashMap<String, (bool, DashMap<String, UnboundedSender<Message>>)>;

async fn worker<S: 'static>(conn: WebSocketStream<S>, connections: Arc<State>, is_admin: bool)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let freq_ratelimiter = constants::get_freq_ratelimiter();
    let size_ratelimiter = constants::get_size_ratelimiter();
    let rooms: Arc<DashSet<String>> = Arc::new(DashSet::new());
    let id = uuid::Uuid::new_v4().to_string();

    let (mut write, mut read) = conn.split();
    let (tx, mut rx) = unbounded_channel();

    spawn(async move {
        while let Some(msg) = rx.recv().await {
            debug!("Sending websocket message: {:?}", msg);
            let _ = write.send(msg).await;
        }
    });

    while let Some(msg) = read.next().await {
        if let Ok(m) = msg {
            let amt = m.len();
            debug!("{:?}", m);
            match from_slice::<model::Payload>(&mut m.clone().into_data()) {
                Ok(payload) => match &payload {
                    model::Payload::Message(msg) => {
                        if !is_admin {
                            let _ = freq_ratelimiter.acquire_one().await;
                            let _ = size_ratelimiter.acquire(amt).await;
                        }

                        if rooms.contains(&msg.room) {
                            let room = connections.get(&msg.room).unwrap();

                            if room.0 && !is_admin {
                                let _ =
                                    tx.send(Message::Text(constants::ROOM_READ_ONLY.to_string()));
                                continue;
                            }

                            for recp in room.1.iter() {
                                if recp.key() != &id {
                                    let _ = recp.value().send(m.clone());
                                }
                            }
                        } else {
                            let _ = tx.send(Message::Text(constants::INVALID_ROOM_MSG.to_string()));
                        }
                    }
                    model::Payload::Command(cmd) => match cmd {
                        model::Command::Join(j) => {
                            if constants::is_valid_room(&j.room)
                                || CONFIG.public_channels.iter().any(|c| c.name == j.room)
                            {
                                rooms.insert(j.room.clone());

                                if let Some(conns) = connections.get(&j.room) {
                                    conns.value().1.insert(id.clone(), tx.clone());
                                } else {
                                    let map = DashMap::new();
                                    map.insert(id.clone(), tx.clone());
                                    connections.insert(j.room.clone(), (false, map));
                                }

                                let _ =
                                    tx.send(Message::text(constants::ROOM_JOIN_MSG.to_string()));
                            } else {
                                let _ = tx.send(Message::Text(
                                    constants::ROOM_NAME_TOO_SHORT.to_string(),
                                ));
                            }
                        }
                        model::Command::Leave(l) => {
                            let was_in_room = rooms.remove(&l.room).is_some();
                            if was_in_room {
                                let conns = connections.get(&l.room);
                                if let Some(c) = conns {
                                    if c.1.len() == 1 {
                                        drop(c);
                                        debug!("Deleting room");
                                        connections.remove(&l.room);
                                    } else {
                                        debug!("Leaving room");
                                        c.1.remove(&id);
                                    }

                                    debug!("Rooms: {:?}", connections);
                                }
                                let _ =
                                    tx.send(Message::Text(constants::ROOM_LEAVE_MSG.to_string()));
                            } else {
                                let _ =
                                    tx.send(Message::Text(constants::INVALID_ROOM_MSG.to_string()));
                            }
                        }
                    },
                },
                Err(e) => {
                    debug!("Error in JSON: {:?}", e);
                    let _ = tx.send(Message::Text(constants::INVALID_JSON_MSG.to_string()));
                }
            }
        }
    }

    debug!("Cleaning up websocket");
    for room in rooms.iter() {
        let conns = connections.get(room.key());
        if let Some(c) = conns {
            if c.1.len() == 1 {
                drop(c);
                debug!("Deleting room");
                connections.remove(room.key());
            } else {
                debug!("Leaving room");
                c.1.remove(&id);
            }

            debug!("Rooms: {:?}", connections);
        }
    }
}

async fn handle_connection(
    raw_stream: TcpStream,
    addr: SocketAddr,
    connections: Arc<State>,
) -> Result<(), Error> {
    info!("Incoming connection from {:?}", addr);
    let mut is_admin = false;
    let auth_callback = |req: &Request, res: Response| {
        let auth = req.headers().get("Authorization");
        if let (Some(header), Some(hash)) = (auth, CONFIG.admin_password_hash.as_ref()) {
            let hasher = Argon2::default();
            let parsed_hash = PasswordHash::new(hash).unwrap();
            if hasher
                .verify_password(header.as_bytes(), &parsed_hash)
                .is_ok()
            {
                is_admin = true;
            }
        }
        Ok(res)
    };

    let ws_stream = accept_hdr_async_with_config(
        raw_stream,
        auth_callback,
        Some(WebSocketConfig {
            accept_unmasked_frames: false,
            max_send_queue: None,
            max_message_size: Some(CONFIG.max_message_size),
            max_frame_size: Some(CONFIG.max_frame_size),
        }),
    )
    .await?;

    worker(ws_stream, connections, is_admin).await;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), IoError> {
    if var("RUST_LOG").is_err() {
        set_var("RUST_LOG", "info");
    }
    env_logger::init();

    let addr: SocketAddr = ([0, 0, 0, 0], CONFIG.port).into();

    let connections: Arc<State> = Arc::new(DashMap::new());

    for room in &CONFIG.public_channels {
        let map = DashMap::new();
        connections.insert(room.name.clone(), (room.read_only, map));
    }

    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    info!("Listening on: {}", addr);

    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(stream, addr, connections.clone()));
    }

    Ok(())
}
