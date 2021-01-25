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
    upgrade, Body, Client, Method, Request, Response, Server, StatusCode,
};
use log::{debug, error, info};
use qstring::QString;
use sha1::{Digest, Sha1};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    spawn,
    sync::{
        broadcast::{channel, Sender},
        mpsc::unbounded_channel,
    },
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{protocol::Role, Message},
    WebSocketStream,
};

use std::{
    env::{set_var, var},
    io::Read,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

mod constants;
mod error;
mod json;
mod model;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type State = DashMap<String, (Sender<Message>, AtomicUsize)>;

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

    connection_handler(conn, rooms, connections, true).await
}

async fn connection_handler<S: 'static>(
    conn: WebSocketStream<S>,
    rooms: Vec<String>,
    connections: Arc<State>,
    relaying: bool,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let freq_ratelimiter = constants::get_freq_ratelimiter();
    let size_ratelimiter = constants::get_size_ratelimiter();

    let (mut write, mut read) = conn.split();
    let (write_handle, mut read_handle) = unbounded_channel();

    spawn(async move {
        while let Some(msg) = read_handle.recv().await {
            debug!("Sending websocket message: {:?}", msg);
            let _ = write.send(msg).await;
        }
    });

    for room in rooms.iter() {
        let mut rx = if let Some(conn) = connections.get(room) {
            let (tx, counter) = conn.value().clone();
            counter.fetch_add(1, Ordering::Relaxed);
            tx.subscribe()
        } else {
            let (tx, rx) = channel(100);
            connections.insert(room.clone(), (tx.clone(), AtomicUsize::new(1)));
            rx
        };

        let wh = write_handle.clone();
        spawn(async move {
            while let Ok(msg) = rx.recv().await {
                let _ = wh.send(msg);
            }
        });
    }

    while let Some(msg) = read.next().await {
        if let Ok(m) = msg {
            let mut data = m.clone().into_data();
            if let Ok(payload) = from_slice::<model::Payload>(&mut data) {
                debug!("Got websocket message: {:?}", payload);
                if !relaying {
                    let _ = freq_ratelimiter.acquire_one().await;
                    let _ = size_ratelimiter.acquire(data.len()).await;
                }
                if rooms.iter().any(|i| i == &payload.room) {
                    let _ = connections.get(&payload.room).unwrap().value().0.send(m);
                }
            }
        }
    }

    debug!("Cleaning up websocket");
    for room in rooms.iter() {
        let conns = connections
            .get(room)
            .expect("Should never be None at this stage");
        debug!("Leaving room");
        let new = conns.1.fetch_sub(1, Ordering::Relaxed);
        if new == 1 {
            debug!("Deleting room");
            connections.remove(room);
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
                let conn = WebSocketStream::from_raw_socket(
                    upgraded,
                    Role::Server,
                    Some(*constants::CONFIG),
                )
                .await;
                if let Err(e) = connection_handler(conn, rooms, connections, false).await {
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
