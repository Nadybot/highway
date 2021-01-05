use async_tungstenite::{
    tungstenite::{
        protocol::{Role, WebSocketConfig},
        Message,
    },
    WebSocketStream,
};
use base64::encode;
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use hyper::{
    header::{
        HeaderValue, CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION,
        UPGRADE,
    },
    service::{make_service_fn, service_fn},
    upgrade,
    upgrade::Upgraded,
    Body, Request, Response, Server, StatusCode,
};
use leaky_bucket::LeakyBucket;
use log::{debug, error, info};
use qstring::QString;
use serde::Deserialize;
#[cfg(not(feature = "simd"))]
use serde_json::{from_slice, from_str};
use sha1::{Digest, Sha1};
#[cfg(feature = "simd")]
use simd_json::{from_slice, from_str};
use tokio::{
    spawn,
    sync::mpsc::{unbounded_channel, UnboundedSender},
    time::Duration,
};
use tokio_util::compat::TokioAsyncReadCompatExt;
use uuid::Uuid;

use std::{
    env::{set_var, var},
    sync::Arc,
};

// A simple type alias so as to DRY.
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type State = DashMap<String, DashMap<String, UnboundedSender<Message>>>;
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Deserialize, Debug)]
struct Payload {
    room: String,
    body: String,
}

/// Handle server-side I/O after HTTP upgraded.
async fn server_upgraded_io(
    upgraded: Upgraded,
    rooms: Vec<String>,
    connections: Arc<State>,
) -> Result<()> {
    // we have an upgraded connection that we can read and
    // write on directly.
    let compat = upgraded.compat();
    let conf = WebSocketConfig {
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
    };
    let stream = WebSocketStream::from_raw_socket(compat, Role::Server, Some(conf)).await;
    let (mut write, mut read) = stream.split();
    let id = Uuid::new_v4().to_string();
    let (tx, mut rx) = unbounded_channel();
    let msg_per_sec = var("MSG_PER_SEC")
        .unwrap_or_else(|_| String::from("10"))
        .parse()
        .unwrap();
    let ratelimiter = LeakyBucket::builder()
        .max(msg_per_sec)
        .tokens(msg_per_sec)
        .refill_interval(Duration::from_secs(1))
        .refill_amount(msg_per_sec)
        .build()
        .unwrap();

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

    spawn(async move {
        while let Some(msg) = read.next().await {
            if let Ok(m) = msg {
                let mut data = m.clone().into_data();
                if let Ok(payload) = from_slice::<Payload>(&mut data) {
                    debug!("Got websocket message: {:?}", payload);
                    if rooms.iter().any(|i| i == &payload.room)
                        && ratelimiter.acquire_one().await.is_ok()
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
    });

    Ok(())
}

/// Our server HTTP handler to initiate HTTP upgrades.
async fn server_upgrade(mut req: Request<Body>, connections: Arc<State>) -> Result<Response<Body>> {
    let path = req.uri().path();

    if path == "/stream" {
        let mut res = Response::new(Body::empty());
        let query = QString::from(req.uri().query().unwrap_or_default());
        let mut rooms = query.get("rooms").unwrap_or_default().to_string();

        let rooms = match from_str::<Vec<String>>(&mut rooms) {
            Ok(val) => val,
            Err(_) => {
                *res.status_mut() = StatusCode::BAD_REQUEST;
                return Ok(res);
            }
        };

        info!("Incoming connection to {:?}", &rooms);

        // Send a 400 to any request that doesn't have
        // an `Upgrade` header.
        if !req.headers().contains_key(UPGRADE)
            || !req.headers().contains_key(SEC_WEBSOCKET_KEY)
            || rooms.is_empty()
        {
            *res.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(res);
        }

        let upgrade = req.headers().get(UPGRADE).unwrap();

        if upgrade.to_str().unwrap() != "websocket" {
            *res.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(res);
        }

        let key = req.headers().get(SEC_WEBSOCKET_KEY).unwrap();
        let real_key = encode(Sha1::digest(
            format!("{}{}", key.to_str().unwrap(), GUID).as_bytes(),
        ));

        // Setup a future that will eventually receive the upgraded
        // connection and talk a new protocol, and spawn the future
        // into the runtime.
        //
        // Note: This can't possibly be fulfilled until the 101 response
        // is returned below, so it's better to spawn this future instead
        // waiting for it to complete to then return a response.
        spawn(async move {
            match upgrade::on(&mut req).await {
                Ok(upgraded) => {
                    if let Err(e) = server_upgraded_io(upgraded, rooms, connections).await {
                        eprintln!("server websocket io error: {}", e)
                    };
                }
                Err(e) => eprintln!("upgrade error: {}", e),
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
    } else {
        let mut res = Response::new(Body::empty());
        *res.status_mut() = StatusCode::NOT_FOUND;
        Ok(res)
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

    let make_service = make_service_fn(move |_| {
        let conns = connections.clone();

        async move { Ok::<_, hyper::Error>(service_fn(move |req| server_upgrade(req, conns.clone()))) }
    });

    let server = Server::bind(&addr).serve(make_service);

    info!("Listening on port {}", port);

    if let Err(e) = server.await {
        error!("Server error: {}", e);
    }
}
