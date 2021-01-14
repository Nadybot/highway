use crate::json::{from_slice, from_str};

use base64::encode;
use bytes::Buf;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use hyper::{
    body::aggregate,
    header::{
        HeaderValue, CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION,
        UPGRADE,
    },
    service::{make_service_fn, service_fn},
    upgrade,
    upgrade::Upgraded,
    Body, Client, Method, Request, Response, Server, StatusCode,
};
use log::{debug, error, info};
use qstring::QString;
use sha1::{Digest, Sha1};
use tokio::{
    spawn,
    sync::mpsc::{unbounded_channel, UnboundedSender},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{protocol::Role, Message},
    WebSocketStream,
};
use uuid::Uuid;

use std::{
    env::{set_var, var},
    io::Read,
    sync::Arc,
};

mod constants;
mod error;
mod json;
mod model;

// A simple type alias so as to DRY.
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type State = DashMap<String, DashMap<String, UnboundedSender<Message>>>;

async fn relay_from(addr: String, connections: Arc<State>) -> Result<()> {
    let http_client = Client::new();
    let body = aggregate(
        http_client
            .get(format!("http://{}/public-channels", addr).parse()?)
            .await?,
    )
    .await?;
    let mut rooms = String::new();
    body.reader().read_to_string(&mut rooms)?;
    let query = QString::new(vec![("rooms", &rooms)]);
    let (conn, _) = connect_async(format!("ws://{}/stream?{}", addr, query)).await?;
    let rooms: Vec<String> = from_str(&mut rooms)?;
    let id = Uuid::new_v4().to_string();

    let (mut write, mut read) = conn.split();
    let (tx, mut rx) = unbounded_channel();

    for room in rooms.iter() {
        if let Some(conns) = connections.get(room) {
            conns.value().insert(id.clone(), tx.clone());
        } else {
            let map = DashMap::new();
            map.insert(id.clone(), tx.clone());
            connections.insert(room.clone(), map);
        }
    }

    spawn(async move {
        while let Some(msg) = rx.recv().await {
            debug!("Sending websocket message: {:?}", msg);
            let _ = write.send(msg).await;
        }
    });

    while let Some(msg) = read.next().await {
        if let Ok(m) = msg {
            let mut data = m.clone().into_data();
            if let Ok(payload) = from_slice::<model::Payload>(&mut data) {
                debug!("Got websocket message: {:?}", payload);
                if rooms.iter().any(|i| i == &payload.room) {
                    for recp in connections.get(&payload.room).unwrap().value() {
                        if recp.key() != &id {
                            let _ = recp.value().send(m.clone());
                        }
                    }
                }
            }
        }
    }

    debug!("Cleaning up websocket");
    // Cleanup handler
    for room in rooms.iter() {
        let conns = connections.get(room);
        if let Some(c) = conns {
            if c.len() == 1 {
                drop(c);
                debug!("Deleting room");
                connections.remove(room);
            } else {
                debug!("Leaving room");
                c.value().remove(&id);
            }

            debug!("Rooms: {:?}", connections);
        }
    }

    Ok(())
}

async fn connection_handler(
    upgraded: Upgraded,
    rooms: Vec<String>,
    connections: Arc<State>,
) -> Result<()> {
    let stream =
        WebSocketStream::from_raw_socket(upgraded, Role::Server, Some(*constants::CONFIG)).await;
    let (mut write, mut read) = stream.split();
    let id = Uuid::new_v4().to_string();
    let (tx, mut rx) = unbounded_channel();
    let freq_ratelimiter = constants::get_freq_ratelimiter();
    let size_ratelimiter = constants::get_size_ratelimiter();

    for room in rooms.iter() {
        if let Some(conns) = connections.get(room) {
            conns.value().insert(id.clone(), tx.clone());
        } else {
            let map = DashMap::new();
            map.insert(id.clone(), tx.clone());
            connections.insert(room.clone(), map);
        }
    }

    spawn(async move {
        while let Some(msg) = rx.recv().await {
            debug!("Sending websocket message: {:?}", msg);
            let _ = write.send(msg).await;
        }
    });

    while let Some(msg) = read.next().await {
        if let Ok(m) = msg {
            let mut data = m.clone().into_data();
            if let Ok(payload) = from_slice::<model::Payload>(&mut data) {
                debug!("Got websocket message: {:?}", payload);
                if rooms.iter().any(|i| i == &payload.room)
                    && freq_ratelimiter.acquire_one().await.is_ok()
                    && size_ratelimiter.acquire(data.len()).await.is_ok()
                {
                    for recp in connections.get(&payload.room).unwrap().value() {
                        if recp.key() != &id {
                            let _ = recp.value().send(m.clone());
                        }
                    }
                }
            }
        }
    }

    debug!("Cleaning up websocket");
    // Cleanup handler
    for room in rooms.iter() {
        let conns = connections.get(room);
        if let Some(c) = conns {
            if c.len() == 1 {
                drop(c);
                debug!("Deleting room");
                connections.remove(room);
            } else {
                debug!("Leaving room");
                c.value().remove(&id);
            }

            debug!("Rooms: {:?}", connections);
        }
    }

    Ok(())
}

/// Our server HTTP handler to initiate HTTP upgrades.
async fn handle_connection(
    mut req: Request<Body>,
    connections: Arc<State>,
) -> Result<Response<Body>> {
    let mut res = Response::new(Body::empty());
    let query = QString::from(req.uri().query().unwrap_or_default());
    let mut rooms = query.get("rooms").unwrap_or_default().to_string();

    let rooms = from_str::<Vec<String>>(&mut rooms)?;
    if rooms.iter().any(|r| !constants::is_valid_room(r)) {
        return Err(Box::new(error::RequestError::InvalidRoom));
    }

    info!("Incoming connection to {:?}", &rooms);

    // Send a 400 to any request that doesn't have
    // an `Upgrade` header.
    if !req.headers().contains_key(UPGRADE)
        || !req.headers().contains_key(SEC_WEBSOCKET_KEY)
        || rooms.is_empty()
    {
        return Err(Box::new(error::RequestError::NotWebsocket));
    }

    let upgrade = req.headers().get(UPGRADE).unwrap();

    if upgrade.to_str().unwrap() != "websocket" {
        return Err(Box::new(error::RequestError::NotWebsocket));
    }

    let key = req.headers().get(SEC_WEBSOCKET_KEY).unwrap();
    let real_key = encode(Sha1::digest(
        format!("{}{}", key.to_str().unwrap(), constants::GUID).as_bytes(),
    ));

    // Start the upgrade handler
    spawn(async move {
        match upgrade::on(&mut req).await {
            Ok(upgraded) => {
                if let Err(e) = connection_handler(upgraded, rooms, connections).await {
                    error!("Server websocket io error: {}", e)
                };
            }
            Err(e) => error!("Upgrade error: {}", e),
        }
    });

    *res.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    res.headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    res.headers_mut()
        .insert(UPGRADE, HeaderValue::from_static("websocket"));
    res.headers_mut().insert(
        SEC_WEBSOCKET_ACCEPT,
        HeaderValue::from_str(&real_key).unwrap(),
    );
    res.headers_mut()
        .insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
    Ok(res)
}

async fn request_handler(req: Request<Body>, connections: Arc<State>) -> Result<Response<Body>> {
    info!("{} request to {}", req.method(), req.uri().path());

    match (req.method(), req.uri().path()) {
        (&Method::GET, "/public-channels") => Ok(Response::new(Body::from(
            &*constants::PUBLIC_CHANNELS_SERIALIZED.as_slice(),
        ))),
        (&Method::GET, "/stream") => match handle_connection(req, connections).await {
            // Err means the request was invalid and not a websocket connection
            Ok(r) => Ok(r),
            Err(_) => Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::empty())
                .unwrap()),
        },
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap()),
    }
}

#[tokio::main]
async fn main() {
    if var("RUST_LOG").is_err() {
        set_var("RUST_LOG", "info");
    }
    env_logger::init();

    let port: u16 = var("PORT")
        .unwrap_or_else(|_| String::from("3333"))
        .parse()
        .unwrap();

    let addr = ([0, 0, 0, 0], port).into();

    let connections: Arc<State> = Arc::new(DashMap::new());

    if let Ok(s) = var("RELAY_SOURCE") {
        let conns = connections.clone();
        spawn(async move {
            if let Err(e) = relay_from(s, conns).await {
                error!("Relay failed: {}", e);
            };
        });
    }

    let make_service = make_service_fn(move |_| {
        let conns = connections.clone();

        async move { Ok::<_, hyper::Error>(service_fn(move |req| request_handler(req, conns.clone()))) }
    });

    let server = Server::bind(&addr).serve(make_service);

    info!("Listening on port {}", port);

    if let Err(e) = server.await {
        error!("Server error: {}", e);
    }
}
