use crate::json::from_slice;

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
    accept_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Error, Message},
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

type State = DashMap<String, DashMap<String, UnboundedSender<Message>>>;

async fn worker<S: 'static>(conn: WebSocketStream<S>, connections: Arc<State>)
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
                        let _ = freq_ratelimiter.acquire_one().await;
                        let _ = size_ratelimiter.acquire(amt).await;

                        if rooms.contains(&msg.room) {
                            for recp in connections.get(&msg.room).unwrap().value() {
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
                            if constants::is_valid_room(&j.room) {
                                rooms.insert(j.room.clone());

                                if let Some(conns) = connections.get(&j.room) {
                                    conns.value().insert(id.clone(), tx.clone());
                                } else {
                                    let map = DashMap::new();
                                    map.insert(id.clone(), tx.clone());
                                    connections.insert(j.room.clone(), map);
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
                                    if c.len() == 1 {
                                        drop(c);
                                        debug!("Deleting room");
                                        connections.remove(&l.room);
                                    } else {
                                        debug!("Leaving room");
                                        c.value().remove(&id);
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
            if c.len() == 1 {
                drop(c);
                debug!("Deleting room");
                connections.remove(room.key());
            } else {
                debug!("Leaving room");
                c.value().remove(&id);
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
    let ws_stream = accept_async_with_config(
        raw_stream,
        Some(WebSocketConfig {
            accept_unmasked_frames: false,
            max_send_queue: None,
            max_message_size: Some(
                var("MAX_MESSAGE_SIZE")
                    .unwrap_or_else(|_| String::from("1048576"))
                    .parse()
                    .unwrap(),
            ),
            max_frame_size: Some(
                var("MAX_FRAME_SIZE")
                    .unwrap_or_else(|_| String::from("1048576"))
                    .parse()
                    .unwrap(),
            ),
        }),
    )
    .await?;

    worker(ws_stream, connections).await;

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

    let connections: Arc<State> = Arc::new(DashMap::new());

    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    info!("Listening on: {}", addr);

    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(stream, addr, connections.clone()));
    }

    Ok(())
}
