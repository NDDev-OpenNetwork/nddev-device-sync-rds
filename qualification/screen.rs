use crate::adapter::{OpenView, RdsScreen, ScreenFrame, ScreenView};
use ed25519_dalek::SigningKey;
use nddev_device_sync_domain::screen::{ScreenError, ViewLease};
use rds_agent::{Agent, AgentPolicy};
use rds_client::local::{Client, Prepared, Server};
use rds_core::{
    ServiceKind,
    grant::{Grant, GrantConstraints},
};
use rds_net::{Backend, Endpoint, EndpointConfig, Ticket, Transports, bind_endpoint};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use x11rb::{
    connection::Connection,
    protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, CreateWindowAux, WindowClass},
    rust_connection::RustConnection,
};
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

struct Marker {
    connection: RustConnection,
    window: u32,
}
impl Marker {
    fn new() -> Self {
        assert_eq!(
            std::env::var("NDS_OWNED_XVFB").as_deref(),
            Ok("1"),
            "requires the owned Xvfb runner"
        );
        let auth = std::env::var_os("XAUTHORITY").expect("owned X authority required");
        assert!(std::path::Path::new(&auth).is_file());
        let (connection, _) = RustConnection::connect(None).unwrap();
        let root = connection.setup().roots[0].root;
        let window = connection.generate_id().unwrap();
        connection
            .create_window(
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
        connection.map_window(window).unwrap().check().unwrap();
        connection.flush().unwrap();
        Self { connection, window }
    }
    fn color(&self, pixel: u32) {
        self.connection
            .change_window_attributes(
                self.window,
                &ChangeWindowAttributesAux::new().background_pixel(pixel),
            )
            .unwrap()
            .check()
            .unwrap();
        self.connection
            .clear_area(false, self.window, 0, 0, 0, 0)
            .unwrap()
            .check()
            .unwrap();
        self.connection.flush().unwrap();
    }
}
impl Drop for Marker {
    fn drop(&mut self) {
        let _ = self.connection.destroy_window(self.window);
        let _ = self.connection.flush();
    }
}
struct Fixture {
    root: tempfile::TempDir,
    local: Endpoint,
    remote: Endpoint,
    manager: Server,
    agent: Arc<Agent>,
    task: JoinHandle<()>,
    issuer: SigningKey,
    policy: AgentPolicy,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new(backend: Backend) -> Self {
        let cfg = || EndpointConfig {
            backend,
            discovery: false,
            transports: Transports::DirectOnly,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        };
        let local = bind_endpoint(cfg()).await.unwrap();
        let remote = bind_endpoint(cfg()).await.unwrap();
        let issuer = SigningKey::from_bytes(&rand::random());
        let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
        policy.allow.insert(local.id());
        policy.issuers.insert(issuer.verifying_key().to_bytes());
        policy.services = Some(BTreeSet::from([ServiceKind::Desktop]));
        policy.use_local_revocations();
        let agent = Arc::new(Agent::new(remote.clone(), policy.clone()));
        let serving = agent.clone();
        let task = tokio::spawn(async move {
            let _ = serving.run().await;
        });
        let root = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let prepared = Prepared::bind(root.path().join("control")).await.unwrap();
        let manager = Server::start(Some(prepared), local.clone(), None);
        Self {
            root,
            local,
            remote,
            manager,
            agent,
            task,
            issuer,
            policy,
        }
    }
    fn adapter(&self) -> RdsScreen {
        RdsScreen::new(self.root.path().join("control")).unwrap()
    }
    fn client(&self) -> Client {
        Client::new(self.root.path().join("control"))
    }
    fn request(&self, lifetime_ms: u64, scopes: Vec<ServiceKind>) -> OpenView {
        let grant = Grant::issue(
            &self.issuer,
            *self.local.id().as_bytes(),
            *self.remote.id().as_bytes(),
            rand::random(),
            scopes,
            Duration::from_secs(30),
            GrantConstraints {
                displays: Some(vec![0]),
                max_bps: Some(8_000_000),
                ..Default::default()
            },
        );
        OpenView {
            target: Ticket::of(&self.remote).to_string(),
            peer: *self.remote.id().as_bytes(),
            issuer: self.issuer.verifying_key().to_bytes(),
            grant,
            lease: ViewLease::new(0, 30, 480, now() + lifetime_ms, now()).unwrap(),
        }
    }
    async fn close(&mut self) {
        self.manager.close().await.unwrap();
        self.local.close().await;
        self.remote.close().await;
        self.task.abort();
    }
}
async fn color(view: &mut ScreenView, red: bool) -> ScreenFrame {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let frame = view.next_frame().await.unwrap();
            let (w, h, stride) = frame.dimensions();
            assert_eq!((w, h), (640, 480));
            let bytes = frame.pixels().unwrap();
            let p = &bytes[(100 * stride + 100 * 4) as usize..][..4];
            if (red && p[2] > 180 && p[0] < 70) || (!red && p[0] > 180 && p[2] < 70) {
                return frame;
            }
        }
    })
    .await
    .expect("actual X11 marker pixels never reached decoder")
}
async fn ended(view: &ScreenView) -> ScreenError {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some(error) = view.status() {
                return error;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("view lifetime did not terminate")
}

#[tokio::test]
async fn cancelled_frames_are_inaccessible_before_the_worker_runs_again() {
    let _marker = Marker::new();
    let mut fixture = Fixture::new(Backend::Iroh).await;
    let cancellation = CancellationToken::new();
    let mut view = fixture
        .adapter()
        .open(
            fixture.request(10_000, vec![ServiceKind::DesktopView]),
            cancellation.clone(),
        )
        .await
        .unwrap();
    let frame = color(&mut view, true).await;
    cancellation.cancel();
    // This current-thread runtime cannot schedule the supervisor between these
    // synchronous calls. Access must honor the cancellation token directly.
    assert!(frame.pixels().is_none());
    assert!(matches!(
        view.next_frame().await,
        Err(ScreenError::Cancelled)
    ));
    view.close().await;
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_x11_pixels_cross_managed_quic_and_cancel_under_retention_pressure() {
    let marker = Marker::new();
    for backend in [Backend::Iroh, Backend::Noq] {
        marker.color(0xff0000);
        let mut fixture = Fixture::new(backend).await;
        let adapter = fixture.adapter();
        let token = CancellationToken::new();
        let request = fixture.request(20_000, vec![ServiceKind::DesktopView]);
        let mut view = adapter.open(request, token.clone()).await.unwrap();
        let first = color(&mut view, true).await;
        let before = fixture.client().snapshot().await.unwrap();
        let another = fixture.request(10_000, vec![ServiceKind::DesktopView]);
        assert!(matches!(
            adapter.open(another, CancellationToken::new()).await,
            Err(ScreenError::Busy)
        ));
        marker.color(0x0000ff);
        let second = color(&mut view, false).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), view.next_frame())
                .await
                .is_err(),
            "retained frame limit was bypassed"
        );
        token.cancel();
        assert_eq!(ended(&view).await, ScreenError::Cancelled);
        assert!(first.pixels().is_none() && second.pixels().is_none());
        assert!(matches!(
            view.next_frame().await,
            Err(ScreenError::Cancelled)
        ));
        view.close().await;
        let after = fixture.client().snapshot().await.unwrap();
        assert_eq!(before.generation, after.generation);
        assert_eq!(before.selected, after.selected);
        assert_eq!(
            after.sessions.len(),
            1,
            "closing one view destroyed the native manager connection"
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while fixture.agent.active_streams() != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        fixture.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grant_scope_rejection_and_native_revocation_end_real_video() {
    let _marker = Marker::new();
    let mut fixture = Fixture::new(Backend::Iroh).await;
    let adapter = fixture.adapter();
    for scopes in [
        vec![ServiceKind::Desktop],
        vec![ServiceKind::DesktopView, ServiceKind::DesktopControl],
        vec![ServiceKind::DesktopControl],
    ] {
        assert!(matches!(
            adapter
                .open(fixture.request(10_000, scopes), CancellationToken::new())
                .await,
            Err(ScreenError::Denied)
        ));
    }
    assert!(
        fixture
            .client()
            .snapshot()
            .await
            .unwrap()
            .sessions
            .is_empty()
    );
    let request = fixture.request(20_000, vec![ServiceKind::DesktopView]);
    let grant_id = request.grant.id().unwrap();
    let mut view = adapter
        .open(request, CancellationToken::new())
        .await
        .unwrap();
    let first = color(&mut view, true).await;
    fixture.policy.revoke(grant_id);
    assert!(matches!(
        ended(&view).await,
        ScreenError::Closed | ScreenError::Unavailable
    ));
    assert!(first.pixels().is_none());
    view.close().await;
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expiry_closes_without_a_polling_consumer() {
    let _marker = Marker::new();
    let mut fixture = Fixture::new(Backend::Iroh).await;
    let adapter = fixture.adapter();
    let mut view = adapter
        .open(
            fixture.request(2500, vec![ServiceKind::DesktopView]),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let first = color(&mut view, true).await;
    assert_eq!(ended(&view).await, ScreenError::Expired);
    assert!(first.pixels().is_none());
    view.close().await;
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_owner_rejects_untrusted_issuer_and_wrong_audience() {
    let _marker = Marker::new();
    let mut fixture = Fixture::new(Backend::Iroh).await;
    let adapter = fixture.adapter();
    let mut wrong = fixture.request(10_000, vec![ServiceKind::DesktopView]);
    wrong.peer = rand::random();
    assert!(matches!(
        adapter.open(wrong, CancellationToken::new()).await,
        Err(ScreenError::Denied)
    ));
    let other = SigningKey::from_bytes(&rand::random());
    let mut request = fixture.request(10_000, vec![ServiceKind::DesktopView]);
    request.issuer = other.verifying_key().to_bytes();
    request.grant = Grant::issue(
        &other,
        *fixture.local.id().as_bytes(),
        *fixture.remote.id().as_bytes(),
        rand::random(),
        vec![ServiceKind::DesktopView],
        Duration::from_secs(30),
        GrantConstraints::default(),
    );
    assert!(matches!(
        adapter.open(request, CancellationToken::new()).await,
        Err(ScreenError::Denied)
    ));
    assert!(
        fixture
            .client()
            .snapshot()
            .await
            .unwrap()
            .sessions
            .is_empty()
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_pinned_legacy_manager_refuses_the_v5_client() {
    use std::process::{Command, Stdio};
    struct Process(std::process::Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let Some(binary) = std::env::var_os("NDS_SCREEN_LEGACY_AGENT") else {
        panic!("verified legacy binary required");
    };
    let root = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let control = root.path().join("control");
    let mut process = Process(
        Command::new(binary)
            .args([
                "--backend",
                "noq",
                "--no-relay",
                "--bind-address",
                "127.0.0.1:0",
                "--max-connections",
                "4",
                "--max-streams",
                "4",
            ])
            .arg("--key-file")
            .arg(root.path().join("endpoint.key"))
            .arg("--control-dir")
            .arg(&control)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while !control.join("control.sock").exists() {
            assert!(process.0.try_wait().unwrap().is_none());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        Client::new(control).snapshot().await,
        Err(rds_client::local::Error::Rejected(
            rds_core::local::ErrorCode::Version
        ))
    ));
}
