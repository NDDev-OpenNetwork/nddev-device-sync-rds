use crate::wire;
use iroh::endpoint::SendStream;
use n0_future::time::Instant;
use nddev_device_sync_domain::screen::{
    FIRST_FRAME_TIMEOUT_MS, FRAME_SILENCE_TIMEOUT_MS, FRAME_STALE_MS, MAX_ENCODED_FRAME_BYTES,
    RETAINED_FRAMES, ViewLease, frame_layout,
};
use rds_core::{Codec, DesktopHello, FrameHeader, HelloAck, StreamHello, UniHello, grant::Grant};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    time::Duration,
};
use wasm_bindgen::prelude::*;
fn error(code: &str) -> JsValue {
    JsValue::from_str(code)
}
fn now() -> u64 {
    js_sys::Date::now() as u64
}
struct State {
    closed: Cell<bool>,
    busy: Cell<bool>,
    retained: Cell<u8>,
    expires: Cell<u64>,
    deadline: Cell<Option<Instant>>,
    expired: Cell<bool>,
    next: Cell<Option<u64>>,
    waiting_key: Cell<bool>,
}
impl State {
    fn check_live(&self) -> Result<(), JsValue> {
        if self.expired.get() {
            return Err(error("expired"));
        }
        if self.closed.get() {
            return Err(error("cancelled"));
        }
        let deadline = self.deadline.get().ok_or(error("not_open"))?;
        if Instant::now() >= deadline || now() >= self.expires.get() {
            // Observed expiry is terminal, even if wall time moves backwards
            // or the browser has not dispatched the expiry timer yet.
            self.expired.set(true);
            self.closed.set(true);
            return Err(error("expired"));
        }
        Ok(())
    }
}
struct Busy(Rc<State>);
impl Drop for Busy {
    fn drop(&mut self) {
        self.0.busy.set(false);
    }
}
#[wasm_bindgen]
pub struct BrowserView {
    endpoint: iroh::Endpoint,
    state: Rc<State>,
    connection: RefCell<Option<iroh::endpoint::Connection>>,
    control: tokio::sync::Mutex<Option<SendStream>>,
    events: tokio::sync::Mutex<Option<iroh::endpoint::RecvStream>>,
    route: Cell<[u8; 16]>,
    expiry_task: RefCell<Option<futures_util::future::AbortHandle>>,
}
#[wasm_bindgen]
impl BrowserView {
    #[wasm_bindgen(js_name=create)]
    pub async fn create(relay: String) -> Result<BrowserView, JsValue> {
        let relay = relay
            .parse::<iroh::RelayUrl>()
            .map_err(|_| error("invalid_relay"))?;
        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Custom(iroh::RelayMap::from(relay)))
            .alpns(vec![rds_core::ALPN.to_vec()])
            .bind()
            .await
            .map_err(|_| error("endpoint_bind_failed"))?;
        Ok(Self {
            endpoint,
            state: Rc::new(State {
                closed: Cell::new(false),
                busy: Cell::new(false),
                retained: Cell::new(0),
                expires: Cell::new(0),
                deadline: Cell::new(None),
                expired: Cell::new(false),
                next: Cell::new(None),
                waiting_key: Cell::new(true),
            }),
            connection: RefCell::new(None),
            control: tokio::sync::Mutex::new(None),
            events: tokio::sync::Mutex::new(None),
            route: Cell::new(rand::random()),
            expiry_task: RefCell::new(None),
        })
    }
    pub fn is_current(&self) -> bool {
        self.state.check_live().is_ok()
    }
    pub fn public_id(&self) -> String {
        self.endpoint.id().to_string()
    }
    /// Purpose-bound proof from the existing endpoint owner. No key leaves Rust.
    /// The authority must also verify its NDS device proof and stored challenge.
    pub fn endpoint_binding_proof(&self, challenge: Vec<u8>) -> Result<Vec<u8>, JsValue> {
        const DOMAIN: &[u8] = b"NDS-RDS-ENDPOINT-BINDING-V1\0";
        if self.state.closed.get() {
            return Err(error("cancelled"));
        }
        if !challenge.starts_with(DOMAIN)
            || challenge.len() < DOMAIN.len() + 32
            || challenge.len() > 2048
        {
            return Err(error("invalid_binding_challenge"));
        }
        Ok(self
            .endpoint
            .secret_key()
            .sign(&challenge)
            .to_bytes()
            .to_vec())
    }
    pub async fn open(
        &self,
        peer: String,
        relay: String,
        grant: JsValue,
        issuer: Vec<u8>,
        lifetime_ms: u32,
    ) -> Result<(), JsValue> {
        if self.state.closed.get() || self.connection.borrow().is_some() {
            return Err(error("invalid_lifetime"));
        }
        if self.state.busy.replace(true) {
            return Err(error("frame_busy"));
        }
        let _busy = Busy(self.state.clone());
        let peer = peer
            .parse::<iroh::EndpointId>()
            .map_err(|_| error("invalid_peer"))?;
        let relay = relay
            .parse::<iroh::RelayUrl>()
            .map_err(|_| error("invalid_relay"))?;
        let grant: Grant =
            serde_wasm_bindgen::from_value(grant).map_err(|_| error("invalid_grant"))?;
        let issuer: [u8; 32] = issuer.try_into().map_err(|_| error("invalid_issuer"))?;
        let expires = now() + u64::from(lifetime_ms);
        let deadline = Instant::now() + Duration::from_millis(u64::from(lifetime_ms));
        let lease =
            ViewLease::new(0, 30, 480, expires, now()).map_err(|_| error("invalid_lease"))?;
        let verified = grant
            .verify(
                &HashSet::from([issuer]),
                self.endpoint.id().as_bytes(),
                peer.as_bytes(),
                Duration::from_secs(300),
                now() / 1000,
            )
            .map_err(|_| error("grant_denied"))?;
        lease
            .admit_grant(
                verified.permits_desktop_view(),
                verified.permits_desktop_control(),
                verified
                    .payload
                    .services
                    .iter()
                    .any(|s| *s != rds_core::ServiceKind::DesktopView),
                verified.permits_display(0),
                verified.payload.expires_at * 1000,
            )
            .map_err(|_| error("grant_scope_denied"))?;
        let work = async {
            let connection = self
                .endpoint
                .connect(
                    iroh::EndpointAddr::new(peer).with_relay_url(relay),
                    rds_core::ALPN,
                )
                .await
                .map_err(|_| error("peer_connect_failed"))?;
            if connection.remote_id() != peer {
                return Err(error("peer_mismatch"));
            }
            let (mut send, mut recv) = connection
                .open_bi()
                .await
                .map_err(|_| error("auth_stream_failed"))?;
            wire::prioritize_control(&send, &StreamHello::Authz(grant.clone()))
                .map_err(|_| error("control_priority_failed"))?;
            wire::write_frame(&mut send, &StreamHello::Authz(grant))
                .await
                .map_err(|_| error("auth_write_failed"))?;
            let ack: HelloAck = wire::read_frame(&mut recv)
                .await
                .map_err(|_| error("auth_reply_failed"))?;
            if !matches!(ack, HelloAck::Ok) {
                connection.close(0u32.into(), b"denied");
                return Err(error("native_auth_denied"));
            }
            send.finish().map_err(|_| error("auth_finish_failed"))?;
            recv.read_to_end(0)
                .await
                .map_err(|_| error("auth_commit_failed"))?;
            let (mut send, mut recv) = connection
                .open_bi()
                .await
                .map_err(|_| error("desktop_stream_failed"))?;
            wire::write_frame(
                &mut send,
                &StreamHello::DesktopV3 {
                    session: self.route.get(),
                    hello: DesktopHello {
                        display: 0,
                        max_fps: 30,
                        codec: Codec::H264,
                        input_acks: false,
                    },
                    output_height: 480,
                },
            )
            .await
            .map_err(|_| error("desktop_write_failed"))?;
            let ack: HelloAck = wire::read_frame(&mut recv)
                .await
                .map_err(|_| error("desktop_reply_failed"))?;
            if !matches!(ack, HelloAck::Desktop(_)) {
                connection.close(0u32.into(), b"unavailable");
                return Err(error("desktop_unavailable"));
            }
            // Keep the owning control stream alive; no input or clipboard API.
            *self.control.lock().await = Some(send);
            *self.events.lock().await = Some(recv);
            Ok(connection)
        };
        let conn = n0_future::time::timeout(Duration::from_secs(10), work)
            .await
            .map_err(|_| error("open_timeout"))??;
        if self.state.closed.get() {
            conn.close(0u32.into(), b"cancelled");
            return Err(error("cancelled"));
        }
        if Instant::now() >= deadline || now() >= expires {
            self.state.expired.set(true);
            self.state.closed.set(true);
            conn.close(0u32.into(), b"expired");
            self.control.lock().await.take();
            self.events.lock().await.take();
            return Err(error("expired"));
        }
        self.state.expires.set(expires);
        self.state.deadline.set(Some(deadline));
        let expire_conn = conn.clone();
        let expire_state = self.state.clone();
        let (abort, registration) = futures_util::future::AbortHandle::new_pair();
        *self.expiry_task.borrow_mut() = Some(abort);
        wasm_bindgen_futures::spawn_local(async move {
            let _ = futures_util::future::Abortable::new(
                async move {
                    n0_future::time::sleep_until(deadline).await;
                    expire_state.expired.set(true);
                    expire_state.closed.set(true);
                    expire_conn.close(0u32.into(), b"expired");
                },
                registration,
            )
            .await;
        });
        *self.connection.borrow_mut() = Some(conn);
        Ok(())
    }
    pub async fn next_packet(&self) -> Result<EncodedPacket, JsValue> {
        self.state.check_live()?;
        if usize::from(self.state.retained.get()) >= RETAINED_FRAMES
            || self.state.busy.replace(true)
        {
            return Err(error("frame_busy"));
        }
        let _busy = Busy(self.state.clone());
        let conn = self.connection.borrow().clone().ok_or(error("not_open"))?;
        let work = async {
            loop {
                let mut stream = conn
                    .accept_uni()
                    .await
                    .map_err(|_| error("stream_closed"))?;
                let tag: UniHello = wire::read_frame(&mut stream)
                    .await
                    .map_err(|_| error("invalid_route"))?;
                if tag
                    != (UniHello::DesktopFrames {
                        id: self.route.get(),
                    })
                {
                    let _ = stream.stop(0u32.into());
                    continue;
                }
                let header: FrameHeader = wire::read_frame(&mut stream)
                    .await
                    .map_err(|_| error("invalid_header"))?;
                let stride = header.width.checked_mul(4).ok_or(error("frame_limit"))?;
                frame_layout(
                    header.width,
                    header.height,
                    stride,
                    (stride as usize) * (header.height as usize),
                )
                .map_err(|_| error("frame_limit"))?;
                if header.seq == u64::MAX || header.codec != Codec::H264 {
                    return Err(error("invalid_frame"));
                }
                let body = stream
                    .read_to_end(MAX_ENCODED_FRAME_BYTES)
                    .await
                    .map_err(|_| error("payload_limit"))?;
                if body.is_empty() {
                    return Err(error("empty_payload"));
                }
                if self.state.next.get().is_some_and(|next| header.seq < next) {
                    continue;
                }
                if self.state.next.get().is_some_and(|next| header.seq != next) {
                    conn.close(0u32.into(), b"reference gap");
                    return Err(error("reference_gap"));
                }
                if self.state.waiting_key.get() && !header.keyframe {
                    conn.close(0u32.into(), b"keyframe required");
                    return Err(error("keyframe_required"));
                }
                if header.keyframe {
                    self.state.waiting_key.set(false)
                }
                self.state.next.set(Some(header.seq + 1));
                self.state.check_live()?;
                self.state.retained.set(self.state.retained.get() + 1);
                return Ok(EncodedPacket {
                    header,
                    body: RefCell::new(Some(body)),
                    state: self.state.clone(),
                    received: Instant::now(),
                });
            }
        };
        let timeout = if self.state.next.get().is_none() {
            FIRST_FRAME_TIMEOUT_MS
        } else {
            FRAME_SILENCE_TIMEOUT_MS
        };
        let result = n0_future::time::timeout(
            Duration::from_millis(timeout).min(
                self.state
                    .deadline
                    .get()
                    .ok_or(error("not_open"))?
                    .saturating_duration_since(Instant::now()),
            ),
            work,
        )
        .await
        .unwrap_or_else(|_| Err(error("receive_timeout")));
        if result.is_err() {
            self.state.closed.set(true);
            conn.close(0u32.into(), b"receive failed");
        }
        result
    }
    pub async fn close(&self) {
        self.state.closed.set(true);
        if let Some(task) = self.expiry_task.borrow_mut().take() {
            task.abort();
        }
        if let Some(conn) = self.connection.borrow_mut().take() {
            conn.close(0u32.into(), b"cancelled");
        }
        self.control.lock().await.take();
        self.events.lock().await.take();
        self.endpoint.close().await;
    }
}
#[wasm_bindgen]
pub struct EncodedPacket {
    header: FrameHeader,
    body: RefCell<Option<Vec<u8>>>,
    state: Rc<State>,
    received: Instant,
}
#[wasm_bindgen]
impl EncodedPacket {
    pub fn sequence(&self) -> u64 {
        self.header.seq
    }
    pub fn width(&self) -> u32 {
        self.header.width
    }
    pub fn height(&self) -> u32 {
        self.header.height
    }
    pub fn keyframe(&self) -> bool {
        self.header.keyframe
    }
    pub fn take_payload(&self) -> Result<Vec<u8>, JsValue> {
        if let Err(error) = self.state.check_live() {
            self.body.borrow_mut().take();
            return Err(error);
        }
        if self.received.elapsed() >= Duration::from_millis(FRAME_STALE_MS) {
            self.body.borrow_mut().take();
            return Err(error("stale_frame"));
        }
        self.body
            .borrow_mut()
            .take()
            .ok_or(error("payload_already_taken"))
    }
}
impl Drop for EncodedPacket {
    fn drop(&mut self) {
        self.state
            .retained
            .set(self.state.retained.get().saturating_sub(1));
    }
}

#[wasm_bindgen]
pub fn valid_frame_size(width: u32, height: u32, stride: u32, bytes: u32) -> bool {
    frame_layout(width, height, stride, bytes as usize).is_ok()
}

#[wasm_bindgen]
pub fn render_wait_timeout_ms() -> u32 {
    FRAME_SILENCE_TIMEOUT_MS as u32
}

impl Drop for BrowserView {
    fn drop(&mut self) {
        self.state.closed.set(true);
        if let Some(task) = self.expiry_task.borrow_mut().take() {
            task.abort();
        }
        if let Some(connection) = self.connection.borrow_mut().take() {
            connection.close(0u32.into(), b"dropped");
        }
    }
}
