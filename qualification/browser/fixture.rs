use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use ed25519_dalek::SigningKey;
use rds_agent::{Agent, AgentPolicy};
use rds_core::{
    ServiceKind,
    grant::{Grant, GrantConstraints},
};
use rds_net::{Backend, Endpoint, EndpointConfig, Transports};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use x11rb::{
    connection::Connection,
    protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, CreateWindowAux, WindowClass},
    rust_connection::RustConnection,
};
struct Fixture {
    endpoint: Endpoint,
    issuer: SigningKey,
    cap: String,
    assets: PathBuf,
    marker: Mutex<(RustConnection, u32)>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    challenge: Mutex<Option<BindingChallenge>>,
}
struct BindingChallenge {
    subject: rds_net::EndpointId,
    bytes: Vec<u8>,
    deadline: Instant,
    attempts: u8,
}
// This fixture proves only transport-key possession. Production admission also
// requires the existing NDS device signature, session and transactional store.
async fn binding_challenge(
    State(f): State<Arc<Fixture>>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    if headers.get("authorization").and_then(|s| s.to_str().ok())
        != Some(&format!("Bearer {}", f.cap))
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let subject = value["subject"]
        .as_str()
        .ok_or(StatusCode::BAD_REQUEST)?
        .parse::<rds_net::EndpointId>()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let ttl = value["ttl_ms"]
        .as_u64()
        .unwrap_or(10_000)
        .clamp(100, 10_000);
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + ttl;
    let mut bytes = b"NDS-RDS-ENDPOINT-BINDING-V1\0".to_vec();
    bytes.extend_from_slice(&rand::random::<[u8; 32]>());
    bytes.extend_from_slice(subject.as_bytes());
    bytes.extend_from_slice(f.endpoint.id().as_bytes());
    bytes.extend_from_slice(&expires.to_be_bytes());
    *f.challenge.lock().unwrap() = Some(BindingChallenge {
        subject,
        bytes: bytes.clone(),
        deadline: Instant::now() + Duration::from_millis(ttl),
        attempts: 5,
    });
    Ok(Json(json!({"challenge": bytes})))
}
async fn grant(
    State(f): State<Arc<Fixture>>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    if headers.get("authorization").and_then(|s| s.to_str().ok())
        != Some(&format!("Bearer {}", f.cap))
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let peer = value["subject"]
        .as_str()
        .ok_or(StatusCode::BAD_REQUEST)?
        .parse::<rds_net::EndpointId>()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let body = serde_json::from_value::<Vec<u8>>(value["challenge"].clone())
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let signature = serde_json::from_value::<Vec<u8>>(value["signature"].clone())
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let signature =
        ed25519_dalek::Signature::from_slice(&signature).map_err(|_| StatusCode::BAD_REQUEST)?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(peer.as_bytes())
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    {
        let mut stored = f.challenge.lock().unwrap();
        let challenge = stored.as_mut().ok_or(StatusCode::UNAUTHORIZED)?;
        if Instant::now() >= challenge.deadline || challenge.attempts == 0 {
            *stored = None;
            return Err(StatusCode::UNAUTHORIZED);
        }
        challenge.attempts -= 1;
        if challenge.subject != peer
            || challenge.bytes != body
            || key.verify_strict(&body, &signature).is_err()
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        *stored = None;
    }
    let mut task = f.task.lock().unwrap();
    if task.is_some() {
        return Err(StatusCode::CONFLICT);
    }
    let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
    policy.allow.insert(peer);
    policy.issuers.insert(f.issuer.verifying_key().to_bytes());
    policy.services = Some(BTreeSet::from([ServiceKind::Desktop]));
    policy.use_local_revocations();
    let agent = Arc::new(Agent::new(f.endpoint.clone(), policy));
    *task = Some(tokio::spawn(async move {
        let _ = agent.run().await;
    }));
    let grant = Grant::issue(
        &f.issuer,
        *peer.as_bytes(),
        *f.endpoint.id().as_bytes(),
        rand::random(),
        vec![ServiceKind::DesktopView],
        Duration::from_secs(60),
        GrantConstraints {
            displays: Some(vec![0]),
            max_bps: Some(8_000_000),
            ..Default::default()
        },
    );
    Ok(Json(
        json!({"grant":grant,"peer":f.endpoint.id().to_string(),"issuer":f.issuer.verifying_key().to_bytes()}),
    ))
}
async fn color(
    State(f): State<Arc<Fixture>>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> StatusCode {
    if headers.get("authorization").and_then(|s| s.to_str().ok())
        != Some(&format!("Bearer {}", f.cap))
    {
        return StatusCode::UNAUTHORIZED;
    }
    let color = if value["blue"] == true {
        0x0000ff
    } else {
        0xff0000
    };
    let marker = f.marker.lock().unwrap();
    marker
        .0
        .change_window_attributes(
            marker.1,
            &ChangeWindowAttributesAux::new().background_pixel(color),
        )
        .unwrap()
        .check()
        .unwrap();
    marker
        .0
        .clear_area(false, marker.1, 0, 0, 0, 0)
        .unwrap()
        .check()
        .unwrap();
    marker.0.flush().unwrap();
    StatusCode::NO_CONTENT
}
async fn asset(State(f): State<Arc<Fixture>>, Path(path): Path<String>) -> impl IntoResponse {
    if path.split('/').any(|p| p == ".." || p.is_empty()) || path.len() > 256 {
        return (
            StatusCode::BAD_REQUEST,
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            Vec::new(),
        );
    }
    let p = f.assets.join(&path);
    let Ok(p) = p.canonicalize() else {
        return (
            StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            Vec::new(),
        );
    };
    if !p.starts_with(&f.assets) {
        return (
            StatusCode::FORBIDDEN,
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            Vec::new(),
        );
    }
    let mime = if path.ends_with(".wasm") {
        "application/wasm"
    } else if path.ends_with(".js") {
        "text/javascript"
    } else {
        "text/html"
    };
    let Ok(bytes) = tokio::fs::read(p).await else {
        return (
            StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            Vec::new(),
        );
    };
    if bytes.len() > 20_000_000 {
        return (
            StatusCode::BAD_REQUEST,
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            Vec::new(),
        );
    }
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, mime)],
        bytes,
    )
}
async fn overflow(upgrade: axum::extract::ws::WebSocketUpgrade) -> impl IntoResponse {
    upgrade.on_upgrade(|mut socket| async move {
        let _ = socket
            .send(axum::extract::ws::Message::Binary(vec![0; 1048577].into()))
            .await;
        let _ = socket.recv().await;
    })
}
async fn text_overflow(upgrade: axum::extract::ws::WebSocketUpgrade) -> impl IntoResponse {
    upgrade.on_upgrade(|mut socket| async move {
        let _ = socket
            .send(axum::extract::ws::Message::Text("界".repeat(524289).into()))
            .await;
        let _ = socket.recv().await;
    })
}
async fn burst(upgrade: axum::extract::ws::WebSocketUpgrade) -> impl IntoResponse {
    upgrade.on_upgrade(|mut socket| async move {
        for _ in 0..32 {
            if socket
                .send(axum::extract::ws::Message::Binary(vec![0; 1024].into()))
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = socket.recv().await;
    })
}

#[tokio::main]
async fn main() {
    assert_eq!(std::env::var("NDS_OWNED_XVFB").as_deref(), Ok("1"));
    let assets = PathBuf::from(std::env::args().nth(1).unwrap())
        .canonicalize()
        .unwrap();
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(iroh_relay::server::RelayConfig::new(([127, 0, 0, 1], 0)));
    config.quic = None;
    config.metrics_addr = None;
    let relay = iroh_relay::server::Server::spawn(config).await.unwrap();
    let url = format!("http://{}", relay.http_addr().unwrap())
        .parse::<rds_net::RelayUrl>()
        .unwrap();
    let endpoint = rds_net::bind_endpoint(EndpointConfig {
        backend: Backend::Iroh,
        discovery: false,
        relays: vec![url.clone()],
        transports: Transports::RelayOnly,
        ..Default::default()
    })
    .await
    .unwrap();
    let (x, _) = RustConnection::connect(None).unwrap();
    let window = x.generate_id().unwrap();
    let root = x.setup().roots[0].root;
    x.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        320,
        240,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().background_pixel(0xff0000u32),
    )
    .unwrap()
    .check()
    .unwrap();
    x.map_window(window).unwrap().check().unwrap();
    x.flush().unwrap();
    let cap = format!("{:032x}", rand::random::<u128>());
    let fixture = Arc::new(Fixture {
        endpoint,
        issuer: SigningKey::from_bytes(&rand::random()),
        cap: cap.clone(),
        assets,
        marker: Mutex::new((x, window)),
        task: Mutex::new(None),
        challenge: Mutex::new(None),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    println!(
        "{}",
        json!({"origin":origin,"relay":url.to_string(),"cap":cap})
    );
    let router = Router::new()
        .route("/overflow", get(overflow))
        .route("/text-overflow", get(text_overflow))
        .route("/burst", get(burst))
        .route("/binding-challenge", post(binding_challenge))
        .route("/grant", post(grant))
        .route("/color", post(color))
        .route("/assets/{*path}", get(asset))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(fixture);
    axum::serve(listener, router).await.unwrap();
    drop(relay);
}
