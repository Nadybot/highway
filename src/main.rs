use crate::json::from_slice;

use dashmap::DashSet;
use futures_util::{SinkExt, StreamExt};
use log::{debug, info};
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
    accept_async,
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

    let listen_rooms = rooms.clone();
    spawn(async move {
        while let Some(msg) = rx.recv().await {
            debug!("Sending websocket message: {:?}", msg);
            let _ = write.send(msg).await;
        }
    });

    let send_tx = tx.clone();
    spawn(async move {
        while let Ok(msg) = receiver.recv().await {
            if listen_rooms.contains(&msg.message.room) {
                let _ = send_tx.send(msg.tungstenite_message);
            }
        }
    });

    while let Some(msg) = read.next().await {
        if let Ok(m) = msg {
            let amt = m.len();
            debug!("{:?}", m);
            if let Ok(payload) = from_slice::<model::Payload>(&mut m.clone().into_data()) {
                match payload {
                    model::Payload::Message(msg) => {
                        if !relaying {
                            let _ = freq_ratelimiter.acquire_one().await;
                            let _ = size_ratelimiter.acquire(amt).await;
                        }

                        if rooms.contains(&msg.room) {
                            let msg = model::InternalMessage {
                                message: msg,
                                tungstenite_message: m,
                            };
                            let _ = broadcast.send(msg);
                        } else {
                            let _ = tx.send(Message::Text(constants::INVALID_ROOM_MSG.to_string()));
                        }
                    }
                    model::Payload::Command(cmd) => match cmd.cmd {
                        model::CommandType::Join => {
                            if constants::is_valid_room(&cmd.room) {
                                rooms.insert(cmd.room);
                                let _ =
                                    tx.send(Message::text(constants::ROOM_JOIN_MSG.to_string()));
                            } else {
                                let _ =
                                    tx.send(Message::Text(constants::INVALID_ROOM_MSG.to_string()));
                            }
                        }
                        model::CommandType::Leave => {
                            let was_in_room = rooms.remove(&cmd.room).is_some();
                            if was_in_room {
                                let _ =
                                    tx.send(Message::Text(constants::ROOM_LEAVE_MSG.to_string()));
                            } else {
                                let _ =
                                    tx.send(Message::Text(constants::INVALID_ROOM_MSG.to_string()));
                            }
                        }
                        _ => {
                            let _ = tx.send(Message::Text(constants::INVALID_CMD_MSG.to_string()));
                        }
                    },
                }
            } else {
                let _ = tx.send(Message::Text(constants::INVALID_JSON_MSG.to_string()));
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
    let mut ws_stream = accept_async(raw_stream).await?;
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

    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    info!("Listening on: {}", addr);

    // Let's spawn the handling of each connection in a separate task.
    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(stream, addr, broadcast.clone()));
    }

    Ok(())
}
