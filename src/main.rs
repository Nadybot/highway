#![deny(clippy::pedantic)]
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
use std::{
    borrow::Borrow,
    collections::HashSet,
    env::var,
    hash::{Hash, Hasher},
    hint::unreachable_unchecked,
    io::Error as IoError,
    net::{IpAddr, SocketAddr},
    ops::Deref,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use arc_swap::ArcSwap;
use argon2::{
    password_hash::{PasswordHash, PasswordVerifier},
    Argon2,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use bytes::{Bytes, BytesMut};
use config::{Config, Ratelimit};
use dashmap::DashMap;
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use http_body_util::Full;
use hyper::{
    body::Incoming,
    header::{
        HeaderValue, AUTHORIZATION, CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY,
        SEC_WEBSOCKET_VERSION, UPGRADE, USER_AGENT,
    },
    server::conn::http1,
    service::service_fn,
    Error as HyperError, Method, Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use leaky_bucket_lite::LeakyBucket;
use libc::{c_int, sighandler_t, signal, SIGINT, SIGTERM};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use serde_json::{from_slice, to_string, value::RawValue};
use sha1::{Digest, Sha1};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    time::timeout,
};
use tokio_websockets::{Limits, Message, ServerBuilder, WebSocketStream};
use tracing::{debug, error, info};
use tracing_subscriber::{filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

use crate::constants::get_ratelimiter;

mod config;
mod constants;
mod model;

/// A single IP's connection state.
pub struct ClientState {
    /// Number of connections by this client.
    connection_count: usize,
    /// Global ratelimiters.
    ratelimiters: Arc<Ratelimiters>,
}

/// Global state.
pub struct GlobalState {
    /// Configuration.
    pub config: Config,
    /// All rooms currently existing in the server.
    pub rooms: DashMap<String, Room>,
    /// Current connection state from clients.
    pub connections: DashMap<IpAddr, ClientState>,
}

impl GlobalState {
    /// Increases the connection counter for the IP address and returns false if
    /// limit is exceeded, else true.
    fn client_connected(&self, ip: &IpAddr, agent: Option<String>) -> bool {
        if let Some(mut entry) = self.connections.get_mut(ip) {
            if entry.connection_count + 1 > self.config.connections_per_ip {
                return false;
            }

            entry.connection_count += 1;
        } else {
            let ratelimiters = Arc::new(Ratelimiters {
                freq: Some(self.config.get_global_freq_ratelimiter()),
                size: Some(self.config.get_global_size_ratelimiter()),
            });

            self.connections.insert(
                *ip,
                ClientState {
                    connection_count: 1,
                    ratelimiters,
                },
            );
        };

        metrics::increment_counter!("highway_total_connections", "ip" => ip.to_string());
        metrics::increment_gauge!("highway_current_connections", 1.0, "ip" => ip.to_string());

        if let Some(agent) = agent {
            metrics::increment_gauge!("highway_user_agents", 1.0, "agent" => agent);
        }

        true
    }

    /// Decreases the connection counter for the IP address.
    fn client_disconnected(&self, ip: &IpAddr, agent: Option<String>) {
        let mut entry = self
            .connections
            .get_mut(ip)
            .expect("Must have been connected previously");
        entry.connection_count -= 1;

        metrics::decrement_gauge!("highway_current_connections", 1.0, "ip" => ip.to_string());

        if let Some(agent) = agent {
            metrics::decrement_gauge!("highway_user_agents", 1.0, "agent" => agent);
        }

        if entry.connection_count == 0 {
            debug!("Client {} disconnected last connection, removing data", ip);
            drop(entry);
            self.connections.remove(ip);
        }
    }
}

/// Ratelimiters for a client's messages.
struct Ratelimiters {
    /// Size ratelimiter.
    size: Option<LeakyBucket>,
    /// Frequency ratelimiter.
    freq: Option<LeakyBucket>,
}

impl Ratelimiters {
    async fn send_message(&self, payload_size: usize) {
        if let Some(freq) = self.freq.as_ref() {
            let _ = freq.acquire_one().await;
        }

        if let Some(size) = self.size.as_ref() {
            let _ = size.acquire(payload_size as u32).await;
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
    /// The peer's user agent.
    agent: Option<String>,
    /// A list of rooms this client is connected to.
    rooms: DashMap<Room, Ratelimiters>,
    /// Ratelimiters for everything this client sends.
    ratelimiters: Arc<Ratelimiters>,
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
        agent: Option<String>,
        stream: WebSocketStream<S>,
        is_admin: bool,
    ) {
        // Generate a new ID for this peer.
        let id = Uuid::new_v4().to_string();

        // Split the websocket into read and write parts to move them to tasks.
        let (write, read) = stream.split();
        // Create a channel to send messages from anywhere to the websocket.
        let (sender, receiver) = unbounded_channel();

        let ratelimiters = {
            let client_state = global_state
                .connections
                .get(&ip)
                .expect("client is connected already");

            client_state.ratelimiters.clone()
        };

        let peer = Arc::new(Self {
            id,
            is_admin,
            ip,
            agent,
            rooms: DashMap::new(),
            ratelimiters,
            sender,
            global_state,
        });

        peer.hello();

        tokio::spawn(async move { peer.read_task(read).await });
        tokio::spawn(Peer::write_task(ip, write, receiver));
    }

    #[inline]
    fn send(&self, payload: &model::Payload<'_>) {
        let serialized = serde_json::to_string(payload).unwrap();
        let _res = self.sender.send(Message::text(serialized));
    }

    #[inline]
    fn hello(&self) {
        let _res = self.sender.send(Message::text(format!(
            "{{\"type\": \"hello\", \"public_rooms\": {:?}, \"config\": {{\"connections_per_ip\": {}, \"msg_freq_ratelimit\": {}, \"msg_size_ratelimit\": {}, \"max_message_size\": {}}}}}",
            self.global_state.rooms.iter().filter_map(|r| {
                if constants::is_valid_room(&r.name) {
                    None
                } else {
                    Some(r.name.clone())
                }
            }).collect::<Vec<String>>(),
            self.global_state.config.connections_per_ip,
            self.global_state.config.msg_freq_ratelimit,
            self.global_state.config.msg_size_ratelimit,
            self.global_state.config.max_message_size,
        )));
    }

    #[inline]
    fn room_info(
        &self,
        room_name: &str,
        read_only: bool,
        extra_info: Option<&RawValue>,
        msg_freq_ratelimit: Option<&Ratelimit>,
        msg_size_ratelimit: Option<&Ratelimit>,
        members: &[String],
    ) {
        let extra_info = extra_info.map_or("null", |i| (*i).get());

        let text = match (msg_freq_ratelimit, msg_size_ratelimit) {
            (Some(msg_freq_ratelimit), Some(msg_size_ratelimit)) => {
                format!("{{\"type\": \"room_info\", \"room\": \"{room_name}\", \"read_only\": {read_only}, \"extra_info\": {extra_info}, \"msg_freq_ratelimit\": {msg_freq_ratelimit}, \"msg_size_ratelimit\": {msg_size_ratelimit}, \"users\": {members:?}}}")
            }
            (Some(msg_freq_ratelimit), None) => {
                format!("{{\"type\": \"room_info\", \"room\": \"{room_name}\", \"read_only\": {read_only}, \"extra_info\": {extra_info}, \"msg_freq_ratelimit\": {msg_freq_ratelimit}, \"users\": {members:?}}}")
            }
            (None, Some(msg_size_ratelimit)) => {
                format!("{{\"type\": \"room_info\", \"room\": \"{room_name}\", \"read_only\": {read_only}, \"extra_info\": {extra_info}, \"msg_size_ratelimit\": {msg_size_ratelimit}, \"users\": {members:?}}}")
            }
            (None, None) => {
                format!("{{\"type\": \"room_info\", \"room\": \"{room_name}\", \"read_only\": {read_only}, \"extra_info\": {extra_info}, \"users\": {members:?}}}")
            }
        };

        let _res = self.sender.send(Message::text(text));
    }

    /// Leave a room.
    fn leave(&self, payload: model::Payload<'_>) {
        let room_name = payload.room;

        if let Some((room, _)) = self.rooms.remove(room_name) {
            room.unsubscribe(self);
            debug!("{} unsubscribed from room {}", self.id, room_name);
            self.send(&payload.reply_room_leave());
        } else {
            self.send(&payload.reply_invalid_room());
        }
    }

    /// Join a room.
    fn join(&self, payload: model::Payload<'_>) {
        let room_name = payload.room;

        debug!("{} requested to join room {}", self.id, room_name);

        if !(constants::is_valid_room(room_name) || self.global_state.rooms.contains_key(room_name))
        {
            debug!("Room {} is invalid", room_name);
            self.send(&payload.reply_room_name_too_short());
            return;
        }

        if self.rooms.get(room_name).is_some() {
            self.send(&payload.reply_invalid_room());
            return;
        }

        let maybe_room = self.global_state.rooms.get(room_name);
        if let Some(room) = maybe_room {
            debug!("Room {} exists, adding", room_name);

            let metadata = room.metadata.load();

            let room_ratelimiters = Ratelimiters {
                freq: metadata.msg_freq_ratelimit.as_ref().map(get_ratelimiter),
                size: metadata.msg_size_ratelimit.as_ref().map(get_ratelimiter),
            };

            let subscribed = room.subscribed();
            room.subscribe(self);
            self.rooms.insert(room.clone(), room_ratelimiters);

            self.send(&payload.reply_room_join());

            self.room_info(
                room_name,
                metadata.read_only,
                metadata.extra_info.as_deref(),
                metadata.msg_freq_ratelimit.as_ref(),
                metadata.msg_size_ratelimit.as_ref(),
                &subscribed,
            );
        } else {
            debug!("Room {} does not exist, creating", room_name);

            let room_ratelimiters = Ratelimiters {
                freq: None,
                size: None,
            };

            let room = Room::new(room_name.to_string(), false, None, None, None);
            room.subscribe(self);
            self.rooms.insert(room.clone(), room_ratelimiters);
            self.global_state
                .rooms
                .insert(room.inner.name.clone(), room);

            self.send(&payload.reply_room_join());
            self.room_info(room_name, false, None, None, None, &[]);
        }
    }

    /// Send a message in a room.
    async fn send_message(&self, room: &str, payload: &model::Payload<'_>, data_size: usize) {
        let message = Message::text(to_string(payload).expect("Will always be valid JSON"));

        if let Some(entry) = self.rooms.get(room) {
            let room = entry.key();

            if room.metadata.load().read_only && !self.is_admin {
                self.send(&payload.reply_room_read_only());
            } else {
                if !self.is_admin {
                    let ratelimiters = entry.value();
                    ratelimiters.send_message(data_size).await;
                }

                room.broadcast(&message, Some(&self.id));
            }
        } else {
            self.send(&payload.reply_invalid_room());
        }
    }

    /// Handle incoming websocket byte or text data.
    async fn on_message(&self, data: &[u8]) -> bool {
        match from_slice::<model::Payload>(data) {
            Ok(mut payload) => {
                if payload.is_invalid() {
                    debug!("Payload is invalid!");
                    // Reply should still reference any ID set in the request
                    let reply = payload.reply_invalid_msg();
                    let json = serde_json::to_string(&reply).unwrap();

                    let _res = self.sender.send(Message::text(json));
                } else {
                    match payload.kind {
                        model::PayloadKind::Message => {
                            payload.user = &self.id;
                            self.send_message(payload.room, &payload, data.len()).await;
                        }
                        model::PayloadKind::Join => {
                            self.join(payload);
                        }
                        model::PayloadKind::Leave => {
                            self.leave(payload);
                        }
                        model::PayloadKind::Quit => {
                            return true;
                        }
                        _ => unsafe { unreachable_unchecked() }, // Verified by is_invalid
                    }
                }
            }
            Err(e) => {
                debug!("Error in JSON: {:?}", e);
                let _res = self
                    .sender
                    .send(Message::text(constants::INVALID_JSON_MSG.to_string()));
            }
        }

        false
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
        let mut awaiting_pong = false;

        loop {
            let max_wait = if awaiting_pong {
                Duration::from_secs(15)
            } else {
                Duration::from_secs(45)
            };

            match timeout(max_wait, stream.next()).await {
                Ok(Some(Ok(msg))) => {
                    debug!("{} -> {:?}", self.ip, msg);

                    awaiting_pong = false;

                    if msg.is_binary() || msg.is_text() {
                        let payload = msg.into_payload();

                        if !self.is_admin {
                            self.ratelimiters.send_message(payload.len()).await;
                        }

                        let should_quit = self.on_message(&payload).await;

                        if should_quit {
                            break;
                        }
                    }
                }
                Err(_) => {
                    if awaiting_pong {
                        debug!("Did not receive payload from client within 60s, disconnecting");
                        break;
                    }

                    awaiting_pong = true;
                    let msg = Message::ping(BytesMut::new());
                    let _res = self.sender.send(msg);
                }
                _ => break,
            }
        }

        info!("Client from {} disconnected", self.ip);

        for entry in self.rooms.iter() {
            let room = entry.key();
            room.unsubscribe(self);
        }

        self.rooms.clear();

        self.global_state
            .client_disconnected(&self.ip, self.agent.clone());
    }
}

/// A room that clients can connect to.
pub struct RoomInner {
    /// Room name.
    name: String,

    /// Room metadata.
    metadata: ArcSwap<RoomMetaData>,

    /// Subcribed peer senders.
    senders: DashMap<String, UnboundedSender<Message>>,
}

/// Room information that can be modified by hot-reloading the config.
struct RoomMetaData {
    /// Whether or not this room is read-only.
    read_only: bool,
    /// Room extra_info.
    extra_info: Option<Box<RawValue>>,
    /// Ratelimit for message frequency.
    msg_freq_ratelimit: Option<Ratelimit>,
    /// Ratelimit for message size.
    msg_size_ratelimit: Option<Ratelimit>,
}

impl PartialEq for RoomMetaData {
    fn eq(&self, other: &Self) -> bool {
        self.read_only == other.read_only
            && self.extra_info.as_ref().map(|i| i.get())
                == other.extra_info.as_ref().map(|i| i.get())
            && self.msg_freq_ratelimit == other.msg_freq_ratelimit
            && self.msg_size_ratelimit == other.msg_size_ratelimit
    }
}

/// Wrapper around `RoomInner`.
#[derive(PartialEq, Eq, Hash, Clone)]
pub struct Room {
    inner: Arc<RoomInner>,
}

impl Deref for Room {
    type Target = RoomInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Room {
    /// Create a new room.
    fn new(
        name: String,
        read_only: bool,
        extra_info: Option<Box<RawValue>>,
        msg_freq_ratelimit: Option<Ratelimit>,
        msg_size_ratelimit: Option<Ratelimit>,
    ) -> Self {
        Self {
            inner: Arc::new(RoomInner {
                name,
                metadata: ArcSwap::new(Arc::new(RoomMetaData {
                    read_only,
                    extra_info,
                    msg_freq_ratelimit,
                    msg_size_ratelimit,
                })),
                senders: DashMap::new(),
            }),
        }
    }

    /// Announce that a user joined this room.
    fn announce_join(&self, id: &str) {
        let msg = Message::text(format!(
            "{{\"type\": \"join\", \"room\": \"{}\", \"user\": \"{}\"}}",
            self.name, id
        ));

        self.broadcast(&msg, None);
    }

    /// Announce that a user left this room.
    fn announce_leave(&self, id: &str) {
        let msg = Message::text(format!(
            "{{\"type\": \"leave\", \"room\": \"{}\", \"user\": \"{}\"}}",
            self.name, id
        ));

        self.broadcast(&msg, None);
    }

    /// Broadcast a message to all members, optionally providing a peer ID who
    /// sent this message.
    fn broadcast(&self, msg: &Message, sender: Option<&str>) {
        if sender.is_some() {
            metrics::increment_counter!("highway_messages_received");
            metrics::counter!("highway_messages_sent", (self.senders.len() - 1) as u64);
        }

        for send_handle in self.senders.iter() {
            if sender != Some(send_handle.key()) {
                let _res = send_handle.send(msg.clone());
            }
        }
    }

    /// Returns a list of subscribed peer IDs.
    fn subscribed(&self) -> Vec<String> {
        self.senders.iter().map(|e| e.key().to_string()).collect()
    }

    /// Subscribe a peer to this room.
    fn subscribe(&self, peer: &Peer) {
        self.announce_join(&peer.id);

        debug!("Subcribed to messages from room");

        self.senders.insert(peer.id.clone(), peer.sender.clone());

        metrics::gauge!("highway_room_connections", self.senders.len() as f64, "room" => self.name.clone());
    }

    /// Unsubscribe a peer from this room.
    fn unsubscribe(&self, peer: &Peer) {
        self.senders.remove(&peer.id);
        self.announce_leave(&peer.id);

        if self.senders.is_empty() && constants::is_valid_room(&self.name) {
            // All members left
            debug!("Deleting room {}", self.name);
            peer.global_state.rooms.remove(self.inner.name.as_str());
        }

        metrics::gauge!("highway_room_connections", self.senders.len() as f64, "room" => self.name.clone());
    }

    /// Send a new room-info to everyone.
    pub fn resend_room_info(&self) {
        let metadata = self.metadata.load();
        let room_name = &self.name;
        let read_only = metadata.read_only;

        let extra_info = metadata.extra_info.as_ref().map_or("null", |i| (*i).get());

        let mut subscribed: HashSet<String> = HashSet::from_iter(self.subscribed());

        for sender in self.senders.iter() {
            subscribed.remove(sender.key());

            let members = subscribed.iter().collect::<Vec<_>>();

            let text = match (
                metadata.msg_freq_ratelimit.as_ref(),
                metadata.msg_size_ratelimit.as_ref(),
            ) {
                (Some(msg_freq_ratelimit), Some(msg_size_ratelimit)) => {
                    format!("{{\"type\": \"room_info\", \"room\": \"{room_name}\", \"read_only\": {read_only}, \"extra_info\": {extra_info}, \"msg_freq_ratelimit\": {msg_freq_ratelimit}, \"msg_size_ratelimit\": {msg_size_ratelimit}, \"users\": {members:?}}}")
                }
                (Some(msg_freq_ratelimit), None) => {
                    format!("{{\"type\": \"room_info\", \"room\": \"{room_name}\", \"read_only\": {read_only}, \"extra_info\": {extra_info}, \"msg_freq_ratelimit\": {msg_freq_ratelimit}, \"users\": {members:?}}}")
                }
                (None, Some(msg_size_ratelimit)) => {
                    format!("{{\"type\": \"room_info\", \"room\": \"{room_name}\", \"read_only\": {read_only}, \"extra_info\": {extra_info}, \"msg_size_ratelimit\": {msg_size_ratelimit}, \"users\": {members:?}}}")
                }
                (None, None) => {
                    format!("{{\"type\": \"room_info\", \"room\": \"{room_name}\", \"read_only\": {read_only}, \"extra_info\": {extra_info}, \"users\": {members:?}}}")
                }
            };

            let _res = sender.send(Message::text(text));

            subscribed.insert(sender.key().clone());
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

async fn websocket_endpoint_handler(
    addr: SocketAddr,
    mut request: Request<Incoming>,
    global_state: GlobalStateRef,
) -> Result<Response<Full<Bytes>>, HyperError> {
    let mut response = Response::new(Full::default());

    let mut ip = addr.ip();
    let mut is_admin = false;

    if global_state.config.behind_proxy {
        if let Some(header) = request.headers().get("X-Forwarded-For") {
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

    let auth = request.headers().get(AUTHORIZATION);
    if let (Some(header), Some(hash)) = (auth, global_state.config.admin_password_hash.as_ref()) {
        let hasher = Argon2::default();
        let parsed_hash = PasswordHash::new(hash).unwrap();
        if hasher
            .verify_password(header.as_bytes(), &parsed_hash)
            .is_ok()
        {
            is_admin = true;
        }
    }

    if !request.headers().contains_key(UPGRADE)
        || !request.headers().contains_key(SEC_WEBSOCKET_KEY)
    {
        *response.status_mut() = StatusCode::BAD_REQUEST;
        return Ok(response);
    }

    let upgrade = request.headers().get(UPGRADE).unwrap();

    if upgrade.to_str().unwrap() != "websocket" {
        *response.status_mut() = StatusCode::BAD_REQUEST;
        return Ok(response);
    }

    let key = request.headers().get(SEC_WEBSOCKET_KEY).unwrap();

    let mut hasher = Sha1::new();
    hasher.update(key);
    hasher.update(constants::GUID);
    let real_key = STANDARD.encode(hasher.finalize());

    let agent = request
        .headers()
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);

    if !global_state.client_connected(&ip, agent.clone()) {
        debug!("Connection limit exceeded by IP {}, disconnecting", ip);
        *response.status_mut() = StatusCode::FORBIDDEN;
        return Ok(response);
    };

    tokio::spawn(async move {
        match hyper::upgrade::on(&mut request).await {
            Ok(upgraded) => {
                let limits =
                    Limits::default().max_payload_len(Some(global_state.config.max_message_size));

                let ws_stream = ServerBuilder::new()
                    .limits(limits)
                    .serve(TokioIo::new(upgraded));

                Peer::start_from_stream(global_state, ip, agent, ws_stream, is_admin);
            }
            Err(e) => {
                error!("Upgrade error: {}", e);

                global_state.client_disconnected(&ip, agent);
            }
        }
    });

    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    response
        .headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    response
        .headers_mut()
        .insert(UPGRADE, HeaderValue::from_static("websocket"));
    response.headers_mut().insert(
        SEC_WEBSOCKET_ACCEPT,
        HeaderValue::from_str(&real_key).unwrap(),
    );
    response
        .headers_mut()
        .insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));

    Ok(response)
}

async fn http_handler(
    addr: SocketAddr,
    req: Request<Incoming>,
    global_state: GlobalStateRef,
    metrics_handle: Arc<PrometheusHandle>,
) -> Result<Response<Full<Bytes>>, HyperError> {
    debug!("{} request to {}", req.method(), req.uri().path());

    let mut resp = match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => websocket_endpoint_handler(addr, req, global_state).await,
        (&Method::GET, "/metrics") => {
            if let Some(auth_required) = &global_state.config.metrics_token {
                if let Some(auth) = req.headers().get(AUTHORIZATION) {
                    if auth != auth_required {
                        return Ok(Response::builder()
                            .status(StatusCode::FORBIDDEN)
                            .body(Full::default())
                            .unwrap());
                    }
                } else {
                    return Ok(Response::builder()
                        .status(StatusCode::FORBIDDEN)
                        .body(Full::default())
                        .unwrap());
                }
            }

            Ok(Response::builder()
                .body(Full::from(metrics_handle.render()))
                .unwrap())
        }
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::default())
            .unwrap()),
    };

    if let Ok(resp) = resp.as_mut() {
        resp.headers_mut().insert(
            "X-Highway-Version",
            HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
        );
    }

    resp
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

    let log_level = var("RUST_LOG").unwrap_or_else(|_| String::from("info"));
    let level_filter = LevelFilter::from_str(&log_level).unwrap_or(LevelFilter::INFO);
    let fmt_layer = tracing_subscriber::fmt::layer();
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(level_filter)
        .init();

    // Set up metrics collection
    let recorder = PrometheusBuilder::new().build_recorder();
    let metrics_handle = Arc::new(recorder.handle());
    metrics::set_boxed_recorder(Box::new(recorder)).unwrap();

    let mut config = match config::try_load() {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Failed to load config: {e}");
            return Ok(());
        }
    };

    let addr: SocketAddr = ([0, 0, 0, 0], config.port).into();

    let rooms = DashMap::new();

    for room in config.public_channels.drain(..) {
        let room = Room::new(
            room.name,
            room.read_only,
            room.extra_info,
            room.msg_freq_ratelimit,
            room.msg_size_ratelimit,
        );
        rooms.insert(room.inner.name.clone(), room);
    }

    let global_state = Arc::new(GlobalState {
        config,
        rooms,
        connections: DashMap::new(),
    });

    // Start the config hot-reloader
    #[cfg(target_os = "linux")]
    {
        let global_state_copy = global_state.clone();
        tokio::spawn(config::reloader(global_state_copy));
    }

    let listener = TcpListener::bind(&addr).await?;

    info!("Listening on: {}", addr);

    loop {
        let (stream, remote_addr) = match listener.accept().await {
            Ok((stream, remote_addr)) => (stream, remote_addr),
            Err(e) => {
                error!("Failed to accept TCP connection: {e}");
                continue;
            }
        };

        let global_state = global_state.clone();
        let metrics_handle = metrics_handle.clone();

        tokio::spawn(async move {
            if let Err(e) = http1::Builder::new()
                .serve_connection(
                    TokioIo::new(stream),
                    service_fn(move |req: Request<Incoming>| {
                        http_handler(
                            remote_addr,
                            req,
                            global_state.clone(),
                            metrics_handle.clone(),
                        )
                    }),
                )
                .with_upgrades()
                .await
            {
                error!("Failed to serve connection: {e}");
            };
        });
    }
}
