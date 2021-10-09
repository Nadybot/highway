#![feature(once_cell)]
#![feature(option_result_contains)]
#![deny(clippy::pedantic)]
#![allow(clippy::cast_precision_loss)]
use crate::{
    config::CONFIG,
    constants::{get_freq_ratelimiter, get_size_ratelimiter},
    json::{from_slice, to_string},
};

use argon2::{
    password_hash::{PasswordHash, PasswordVerifier},
    Argon2,
};
use dashmap::{DashMap, DashSet};
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use leaky_bucket_lite::LeakyBucket;
use libc::{c_int, sighandler_t, signal, SIGINT, SIGTERM};
use log::{debug, error, info};
use parking_lot::Mutex;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        handshake::server::{ErrorResponse, Request, Response},
        protocol::WebSocketConfig,
        Error, Message,
    },
    WebSocketStream,
};
use uuid::Uuid;

use std::{
    borrow::Borrow,
    collections::HashMap,
    env::{set_var, var},
    hash::{Hash, Hasher},
    io::Error as IoError,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

mod config;
mod constants;
mod json;
mod model;

/// A single IP's connection state.
struct ClientState {
    /// Number of connections by this client.
    connection_count: usize,
    /// Frequency ratelimiter.
    freq_ratelimiter: Arc<LeakyBucket>,
    /// Size ratelimiter.
    size_ratelimiter: Arc<LeakyBucket>,
}

/// Global state.
struct GlobalState {
    /// All rooms currently existing in the server.
    rooms: DashSet<Room>,
    /// Current connection state from clients.
    connections: Mutex<HashMap<IpAddr, ClientState>>,
}

impl GlobalState {
    /// Increases the connection counter for the IP address and returns false if limit is exceeded, else true.
    fn client_connected(&self, ip: &IpAddr) -> bool {
        let mut connections = self.connections.lock();
        if let Some(mut entry) = connections.get_mut(ip) {
            if entry.connection_count + 1 > CONFIG.connections_per_ip {
                return false;
            }

            entry.connection_count += 1;
        } else {
            connections.insert(
                *ip,
                ClientState {
                    connection_count: 1,
                    freq_ratelimiter: Arc::new(get_freq_ratelimiter()),
                    size_ratelimiter: Arc::new(get_size_ratelimiter()),
                },
            );
        }

        true
    }

    /// Decreases the connection counter for the IP address.
    fn client_disconnected(&self, ip: &IpAddr) {
        let mut connections = self.connections.lock();
        let mut entry = connections
            .get_mut(ip)
            .expect("Must have been connected previously");
        entry.connection_count -= 1;

        if entry.connection_count == 0 {
            debug!("Client {} disconnected last connection, removing data", ip);
            connections.remove(ip);
        }
    }
}

/// A client connected to the server
struct Peer {
    /// The peer's ID.
    id: String,
    /// Whether this peer is administrator.
    is_admin: bool,
    /// The peer's IP address.
    ip: IpAddr,
    /// A list of rooms this client is connected to.
    rooms: DashSet<Room>,
    /// Size ratelimiter.
    size_ratelimiter: Arc<LeakyBucket>,
    /// Frequency ratelimiter.
    freq_ratelimiter: Arc<LeakyBucket>,
    /// Handle to send data to the peer.
    sender: UnboundedSender<Message>,
    /// Global state.
    global_state: GlobalStateRef,
}

impl Peer {
    /// Start handling packets from a raw websocket stream.
    fn start_from_stream<S: 'static + AsyncRead + AsyncWrite + Unpin + Send>(
        global_state: GlobalStateRef,
        ip: IpAddr,
        stream: WebSocketStream<S>,
        is_admin: bool,
    ) {
        // Generate a new ID for this peer.
        let id = Uuid::new_v4().to_string();

        // Split the websocket into read and write parts to move them to tasks.
        let (write, read) = stream.split();
        // Create a channel to send messages from anywhere to the websocket.
        let (sender, receiver) = unbounded_channel();

        let (freq_ratelimiter, size_ratelimiter) = {
            let connections = global_state.connections.lock();
            let client_state = connections.get(&ip).expect("client is connected already");
            (
                client_state.freq_ratelimiter.clone(),
                client_state.size_ratelimiter.clone(),
            )
        };

        let peer = Arc::new(Self {
            id,
            is_admin,
            ip,
            rooms: DashSet::new(),
            size_ratelimiter,
            freq_ratelimiter,
            sender,
            global_state,
        });

        peer.hello();

        tokio::spawn(async move { peer.read_task(read).await });
        tokio::spawn(Peer::write_task(ip, write, receiver));
    }

    #[inline]
    fn successfully_joined_the_room(&self) {
        let _res = self
            .sender
            .send(Message::Text(constants::ROOM_JOIN_MSG.to_string()));
    }

    #[inline]
    fn successfully_left_the_room(&self) {
        let _res = self
            .sender
            .send(Message::Text(constants::ROOM_LEAVE_MSG.to_string()));
    }

    #[inline]
    fn room_name_too_short(&self) {
        let _res = self
            .sender
            .send(Message::Text(constants::ROOM_NAME_TOO_SHORT.to_string()));
    }

    #[inline]
    fn invalid_room(&self) {
        let _res = self
            .sender
            .send(Message::Text(constants::INVALID_ROOM_MSG.to_string()));
    }

    #[inline]
    fn room_read_only(&self) {
        let _res = self
            .sender
            .send(Message::Text(constants::ROOM_READ_ONLY.to_string()));
    }

    #[inline]
    fn hello(&self) {
        let _res = self.sender.send(Message::Text(format!(
            "{{\"type\": \"hello\", \"public-rooms\": {:?}, \"config\": {{\"connections_per_ip\": {}, \"msg_per_sec\": {}, \"bytes_per_10_sec\": {}, \"max_message_size\": {}, \"max_frame_size\": {}}}}}",
            CONFIG
                .public_channels
                .iter()
                .map(|channel| channel.name.as_str())
                .collect::<Vec<&str>>(),
            CONFIG.connections_per_ip,
            CONFIG.msg_per_sec,
            CONFIG.bytes_per_10_sec,
            CONFIG.max_message_size,
            CONFIG.max_frame_size
        )));
    }

    #[inline]
    fn room_info(&self, room_name: &str, read_only: bool, members: &[String]) {
        let _res = self.sender.send(Message::Text(format!(
            "{{\"type\": \"room-info\", \"room\": \"{}\", \"read-only\": {}, \"users\": {:?}}}",
            room_name, read_only, members
        )));
    }

    /// Leave a room.
    fn leave(&self, room_name: &str) {
        if let Some(room) = self.rooms.remove(room_name) {
            room.unsubscribe(self);
            debug!("{} unsubscribed from room {}", self.id, room_name);
            self.successfully_left_the_room();
        } else {
            self.invalid_room();
        }
    }

    /// Join a room.
    fn join(&self, room_name: &str) {
        debug!("{} requested to join room {}", self.id, room_name);

        if !(constants::is_valid_room(room_name)
            || CONFIG.public_channels.iter().any(|c| c.name == room_name))
        {
            debug!("Room {} is invalid", room_name);
            self.room_name_too_short();
            return;
        }

        if self.rooms.get(room_name).is_some() {
            self.invalid_room();
            return;
        }

        let maybe_room = self.global_state.rooms.get(room_name);
        if let Some(room) = maybe_room {
            debug!("Room {} exists, adding", room_name);

            let subscribed = room.subscribed();
            room.subscribe(self);
            self.rooms.insert(room.clone());

            self.successfully_joined_the_room();
            self.room_info(room_name, room.inner.read_only, &subscribed);
        } else {
            debug!("Room {} does not exist, creating", room_name);

            let room = Room::new(room_name.to_string(), false);
            room.subscribe(self);
            self.rooms.insert(room.clone());
            self.global_state.rooms.insert(room);

            self.successfully_joined_the_room();
            self.room_info(room_name, false, &[]);
        }
    }

    /// Send a message in a room.
    fn send_message(&self, room: &str, payload: &model::Payload) {
        let message = Message::Text(to_string(payload).expect("Will always be valid JSON"));

        if let Some(room) = self.rooms.get(room) {
            if room.inner.read_only && !self.is_admin {
                self.room_read_only();
            } else {
                room.broadcast(&message, Some(&self.id));
            }
        } else {
            self.invalid_room();
        }
    }

    /// Handle incoming websocket byte or text data.
    fn on_message(&self, mut data: Vec<u8>) {
        match from_slice::<model::Payload>(&mut data) {
            Ok(mut payload) => match payload {
                model::Payload::Message(ref mut msg) => {
                    msg.user = self.id.clone();
                    self.send_message(&msg.room.clone(), &payload);
                }
                model::Payload::Join(join_payload) => {
                    self.join(&join_payload.room);
                }
                model::Payload::Leave(leave_payload) => {
                    self.leave(&leave_payload.room);
                }
            },
            Err(e) => {
                debug!("Error in JSON: {:?}", e);
                let _res = self
                    .sender
                    .send(Message::Text(constants::INVALID_JSON_MSG.to_string()));
            }
        }
    }

    /// Write all packets to the websocket stream.
    async fn write_task<S: AsyncWrite + AsyncRead + Unpin>(
        ip: IpAddr,
        mut sink: SplitSink<WebSocketStream<S>, Message>,
        mut receiver: UnboundedReceiver<Message>,
    ) {
        while let Some(msg) = receiver.recv().await {
            debug!("{} <- {:?}", ip, msg);

            if sink.send(msg).await.is_err() {
                error!("Failed to send message to websocket sink");
                break;
            }
        }

        debug!("Message stream from mpsc channel ended");
    }

    /// Read packets from the websocket stream and handle them.
    async fn read_task<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        mut stream: SplitStream<WebSocketStream<S>>,
    ) {
        while let Some(Ok(msg)) = stream.next().await {
            debug!("{} -> {:?}", self.ip, msg);

            match msg {
                Message::Ping(payload) => {
                    let _res = self.sender.send(Message::Pong(payload));
                }
                Message::Close(_) => break,
                Message::Binary(_) | Message::Text(_) => {
                    let payload = msg.into_data();

                    if !self.is_admin {
                        let _ = self.freq_ratelimiter.acquire_one().await;
                        let _ = self.size_ratelimiter.acquire(payload.len() as f64).await;
                    }

                    self.on_message(payload);
                }
                Message::Pong(_) => {}
            }
        }

        info!("Client from {} disconnected", self.ip);

        for room in self.rooms.iter() {
            room.unsubscribe(self);
        }

        self.rooms.clear();

        self.global_state.client_disconnected(&self.ip);
    }
}

/// A room that clients can connect to.
struct RoomInner {
    /// Room name.
    name: String,
    /// Whether or not this room is read-only.
    read_only: bool,
    /// Subcribed peer senders.
    senders: DashMap<String, UnboundedSender<Message>>,
}

/// Wrapper around `RoomInner` to allow indexing room maps by strings.
#[derive(PartialEq, Eq, Clone, Hash)]
struct Room {
    inner: Arc<RoomInner>,
}

impl Room {
    /// Create a new room.
    fn new(name: String, read_only: bool) -> Self {
        Self {
            inner: Arc::new(RoomInner {
                name,
                read_only,
                senders: DashMap::new(),
            }),
        }
    }

    /// Announce that a user joined this room.
    fn announce_join(&self, id: &str) {
        let msg = Message::text(format!(
            "{{\"type\": \"join\", \"room\": \"{}\", \"user\": \"{}\"}}",
            self.inner.name, id
        ));
        self.broadcast(&msg, None);
    }

    /// Announce that a user left this room.
    fn announce_leave(&self, id: &str) {
        let msg = Message::text(format!(
            "{{\"type\": \"leave\", \"room\": \"{}\", \"user\": \"{}\"}}",
            self.inner.name, id
        ));
        self.broadcast(&msg, None);
    }

    /// Broadcast a message to all members, optionally providing a peer ID who sent this message.
    fn broadcast(&self, msg: &Message, sender: Option<&str>) {
        for send_handle in self.inner.senders.iter() {
            if !sender.contains(send_handle.key()) {
                let _res = send_handle.send(msg.clone());
            }
        }
    }

    /// Returns a list of subscribed peer IDs.
    fn subscribed(&self) -> Vec<String> {
        self.inner
            .senders
            .iter()
            .map(|e| e.key().to_string())
            .collect()
    }

    /// Subscribe a peer to this room.
    fn subscribe(&self, peer: &Peer) {
        self.announce_join(&peer.id);

        debug!("Subcribed to messages from room");

        self.inner
            .senders
            .insert(peer.id.clone(), peer.sender.clone());
    }

    /// Unsubscribe a peer from this room.
    fn unsubscribe(&self, peer: &Peer) {
        self.inner.senders.remove(&peer.id);
        self.announce_leave(&peer.id);

        if self.inner.senders.is_empty() && !self.inner.read_only {
            // All members left
            debug!("Deleting room {}", self.inner.name);
            peer.global_state.rooms.remove(self.inner.name.as_str());
        }
    }
}

impl Hash for RoomInner {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl PartialEq for RoomInner {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for RoomInner {}

impl Borrow<str> for Room {
    fn borrow(&self) -> &str {
        self.inner.name.as_ref()
    }
}

type GlobalStateRef = Arc<GlobalState>;

/// Handle a TCP connection and perform a websocket handshake.
async fn handle_connection(
    raw_stream: TcpStream,
    addr: SocketAddr,
    global_state: GlobalStateRef,
) -> Result<(), Error> {
    let mut ip = addr.ip();

    let mut is_admin = false;
    let auth_callback = |req: &Request, res: Response| {
        // Get the client IP if behind a HTTP proxy
        if CONFIG.behind_proxy {
            if let Some(header) = req.headers().get("X-Forwarded-For") {
                if let Ok(value) = header.to_str() {
                    if let Some(client_ip_str) = value.split(',').next() {
                        if let Ok(client_ip) = client_ip_str.trim().parse() {
                            ip = client_ip;
                        }
                    }
                }
            }
        }

        info!("Incoming connection from {:?}", ip);

        if !global_state.client_connected(&ip) {
            debug!("Connection limit exceeded by IP {}, disconnecting", ip);
            return Err(ErrorResponse::new(Some(String::from(
                "Connection limit exceeded",
            ))));
        };

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

    Peer::start_from_stream(global_state, ip, ws_stream, is_admin);

    Ok(())
}

pub extern "C" fn handler(_: c_int) {
    std::process::exit(0);
}

unsafe fn set_os_handlers() {
    signal(SIGINT, handler as extern "C" fn(_) as sighandler_t);
    signal(SIGTERM, handler as extern "C" fn(_) as sighandler_t);
}

#[tokio::main]
async fn main() -> Result<(), IoError> {
    unsafe { set_os_handlers() };

    if var("RUST_LOG").is_err() {
        set_var("RUST_LOG", "info");
    }
    env_logger::init();

    let addr: SocketAddr = ([0, 0, 0, 0], CONFIG.port).into();
    let global_state = Arc::new(GlobalState {
        rooms: DashSet::new(),
        connections: Mutex::new(HashMap::new()),
    });

    for room in &CONFIG.public_channels {
        let room = Room::new(room.name.clone(), room.read_only);
        global_state.rooms.insert(room);
    }

    let listener = if let Ok(listener) = TcpListener::bind(&addr).await {
        listener
    } else {
        error!("Failed to bind to {}", addr);
        return Ok(());
    };

    info!("Listening on: {}", addr);

    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(stream, addr, global_state.clone()));
    }

    error!("Failed to establish TCP connection in accept");

    Ok(())
}
