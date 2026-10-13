//! Native engine boundary; proposed core contract is supplied by the owning core.
use nddev_device_sync_domain::screen::{self, ScreenError, ViewLease};
use rds_client::local::{Client, ManagedDesktop, ManagedMessage};
use rds_core::{
    Codec, DesktopHello, ServiceKind,
    grant::Grant,
    local::{Command, Reply},
};
use rds_desktop::{
    RawFrame,
    client::{RelayDecoder, RelayOutcome},
};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch},
    task::JoinHandle,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

fn now_ms() -> Result<u64, ScreenError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ScreenError::Unavailable)?
        .as_millis()
        .try_into()
        .map_err(|_| ScreenError::Unavailable)
}

pub struct OpenView {
    pub target: String,
    pub peer: [u8; 32],
    pub issuer: [u8; 32],
    pub grant: Grant,
    pub lease: ViewLease,
}

#[derive(Clone)]
pub struct RdsScreen {
    client: Client,
    admission: Arc<Semaphore>,
}
impl RdsScreen {
    pub fn new(control: PathBuf) -> Result<Self, ScreenError> {
        if !control.is_absolute() {
            return Err(ScreenError::Invalid);
        }
        Ok(Self {
            client: Client::new(control),
            admission: Arc::new(Semaphore::new(screen::MAX_VIEWS)),
        })
    }
    pub async fn open(
        &self,
        request: OpenView,
        cancellation: CancellationToken,
    ) -> Result<ScreenView, ScreenError> {
        if cancellation.is_cancelled() {
            return Err(ScreenError::Cancelled);
        }
        let slot = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| ScreenError::Busy)?;
        let deadline =
            Instant::now() + Duration::from_millis(request.lease.remaining_ms(now_ms()?)?);
        let work = async {
            // Snapshot establishes the exact native local identity and IPC version.
            let local = self
                .client
                .snapshot()
                .await
                .map_err(|_| ScreenError::Unavailable)?;
            let subject = local
                .endpoint
                .parse::<rds_net::EndpointId>()
                .map_err(|_| ScreenError::Invalid)?
                .as_bytes()
                .to_owned();
            let verified = request
                .grant
                .verify(
                    &HashSet::from([request.issuer]),
                    &subject,
                    &request.peer,
                    Duration::from_secs(300),
                    now_ms()? / 1000,
                )
                .map_err(|_| ScreenError::Denied)?;
            request.lease.admit_grant(
                verified.permits_desktop_view(),
                verified.permits_desktop_control(),
                verified
                    .payload
                    .services
                    .iter()
                    .any(|s| *s != ServiceKind::DesktopView),
                verified.permits_display(request.lease.display()),
                verified
                    .payload
                    .expires_at
                    .checked_mul(1000)
                    .ok_or(ScreenError::Denied)?,
            )?;
            let reply = self
                .client
                .request(Command::Connect {
                    target: request.target,
                    grant: Some(Box::new(request.grant)),
                })
                .await
                .map_err(|_| ScreenError::Denied)?;
            let Reply::Connected(session) = reply else {
                return Err(ScreenError::Invalid);
            };
            let snapshot = self
                .client
                .snapshot()
                .await
                .map_err(|_| ScreenError::Unavailable)?;
            let peer = snapshot
                .sessions
                .iter()
                .find(|entry| entry.id == session)
                .ok_or(ScreenError::Unavailable)?;
            if peer
                .peer
                .parse::<rds_net::EndpointId>()
                .map_err(|_| ScreenError::Invalid)?
                .as_bytes()
                != &request.peer
            {
                return Err(ScreenError::Denied);
            }
            let desktop = self
                .client
                .desktop_profile(
                    Some(session),
                    DesktopHello {
                        display: request.lease.display(),
                        max_fps: request.lease.max_fps(),
                        codec: Codec::H264,
                        input_acks: false,
                    },
                    Some(request.lease.output_height()),
                )
                .await
                .map_err(|_| ScreenError::Unavailable)?;
            Ok(desktop)
        };
        let desktop = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(ScreenError::Cancelled),
            _ = tokio::time::sleep_until(deadline) => return Err(ScreenError::Expired),
            result = tokio::time::timeout(Duration::from_secs(10), work) => result.map_err(|_| ScreenError::Timeout)??,
        };
        request.lease.remaining_ms(now_ms()?)?;
        Ok(ScreenView::start(
            desktop,
            cancellation,
            deadline,
            request.lease,
            slot,
        ))
    }
}

pub struct ScreenFrame {
    raw: RawFrame,
    lease: ViewLease,
    deadline: Instant,
    live: Arc<AtomicBool>,
    _retention: OwnedSemaphorePermit,
}
impl ScreenFrame {
    pub fn dimensions(&self) -> (u32, u32, u32) {
        (self.raw.width, self.raw.height, self.raw.stride)
    }
    pub fn pixels(&self) -> Option<&[u8]> {
        (self.live.load(Ordering::Acquire)
            && Instant::now() < self.deadline
            && now_ms().is_ok_and(|now| self.lease.remaining_ms(now).is_ok()))
        .then_some(self.raw.data.as_ref())
    }
}

pub struct ScreenView {
    frames: mpsc::Receiver<ScreenFrame>,
    status: watch::Receiver<Option<ScreenError>>,
    cancellation: CancellationToken,
    live: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
}
impl ScreenView {
    fn start(
        desktop: ManagedDesktop,
        parent: CancellationToken,
        deadline: Instant,
        lease: ViewLease,
        slot: OwnedSemaphorePermit,
    ) -> Self {
        let cancellation = parent.child_token();
        let token = cancellation.clone();
        let live = Arc::new(AtomicBool::new(true));
        let alive = live.clone();
        let (frames_tx, frames) = mpsc::channel(1);
        let (status_tx, status) = watch::channel(None);
        let task = tokio::spawn(async move {
            struct Invalidate(Arc<AtomicBool>);
            impl Drop for Invalidate {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::Release);
                }
            }
            let _invalidate = Invalidate(alive.clone());
            let _slot = slot;
            let result = tokio::select! {
                biased;
                _ = token.cancelled() => Err(ScreenError::Cancelled),
                _ = tokio::time::sleep_until(deadline) => Err(ScreenError::Expired),
                result = receive(desktop, frames_tx, alive.clone(), lease, deadline) => result,
            };
            alive.store(false, Ordering::Release);
            status_tx.send_replace(Some(result.err().unwrap_or(ScreenError::Closed)));
        });
        Self {
            frames,
            status,
            cancellation,
            live,
            task: Some(task),
        }
    }
    pub async fn next_frame(&mut self) -> Result<ScreenFrame, ScreenError> {
        loop {
            if let Some(error) = *self.status.borrow() {
                return Err(error);
            }
            tokio::select! {
                biased;
                changed = self.status.changed() => { changed.map_err(|_| ScreenError::Closed)?; }
                frame = self.frames.recv() => {
                    let frame = frame.ok_or(ScreenError::Closed)?;
                    if self.live.load(Ordering::Acquire) { return Ok(frame); }
                }
            }
        }
    }
    pub fn status(&self) -> Option<ScreenError> {
        *self.status.borrow()
    }
    pub async fn close(mut self) {
        self.live.store(false, Ordering::Release);
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for ScreenView {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn receive(
    mut desktop: ManagedDesktop,
    output: mpsc::Sender<ScreenFrame>,
    live: Arc<AtomicBool>,
    lease: ViewLease,
    deadline: Instant,
) -> Result<(), ScreenError> {
    let budget = Arc::new(Semaphore::new(screen::RETAINED_FRAMES));
    let mut decoder = RelayDecoder::new();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        lease.remaining_ms(now_ms()?)?;
        let message = tokio::select! {
            biased;
            _ = heartbeat.tick() => {
                tokio::time::timeout(Duration::from_secs(2), desktop.heartbeat()).await.map_err(|_| ScreenError::Timeout)?.map_err(|_| ScreenError::Unavailable)?;
                continue;
            }
            next = desktop.recv() => next.map_err(|_| ScreenError::Unavailable)?.ok_or(ScreenError::Closed)?,
        };
        let ManagedMessage::Frame(frame) = message else {
            continue;
        };
        if frame.payload.len() > screen::MAX_ENCODED_FRAME_BYTES {
            return Err(ScreenError::FrameLimit);
        }
        let stride = frame
            .header
            .width
            .checked_mul(4)
            .ok_or(ScreenError::FrameLimit)?;
        let length = (stride as usize)
            .checked_mul(frame.header.height as usize)
            .ok_or(ScreenError::FrameLimit)?;
        screen::frame_layout(frame.header.width, frame.header.height, stride, length)?;
        let retention = budget
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ScreenError::Closed)?;
        let (next, outcome) = decoder
            .push_bounded(frame.header, frame.payload)
            .await
            .map_err(|_| ScreenError::Invalid)?;
        decoder = next;
        match outcome {
            RelayOutcome::Frame(raw) => {
                screen::frame_layout(raw.width, raw.height, raw.stride, raw.data.len())?;
                lease.remaining_ms(now_ms()?)?;
                output
                    .send(ScreenFrame {
                        raw,
                        lease,
                        deadline,
                        live: live.clone(),
                        _retention: retention,
                    })
                    .await
                    .map_err(|_| ScreenError::Closed)?;
            }
            RelayOutcome::NeedIdr => {
                tokio::time::timeout(Duration::from_secs(2), desktop.request_idr())
                    .await
                    .map_err(|_| ScreenError::Timeout)?
                    .map_err(|_| ScreenError::Unavailable)?;
            }
            RelayOutcome::Pending => {}
        }
    }
}
