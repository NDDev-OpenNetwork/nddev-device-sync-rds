# NDDev OpenNetwork · rds

AGPL-3.0-only NDS adapter. [NDDev OpenNetwork](https://nddev.ai).

Read the native manager identity, selection and live sessions through its keyless CLI. No key creation, session mutation, desktop rendering or transport replacement is exposed.

`contracts/module.json` is the canonical compiled-in runtime descriptor;
`module.yaml` identifies its repository package. Caller-owned local/process
capability handles share a bounded agent admission budget. Native responses,
private paths and content are never Debug or telemetry payloads.

Run `just check`, then `just provider-check` with the explicit `NDS_TEST_*`
provider paths used by CI. Acceptance exercises the real pinned native owner
in isolated temporary state. Linux provider acceptance and portable source
checks are distinct; an absent provider is unavailable, not a successful stub.
There is no backup/recovery implementation or private estate configuration.

The pinned provider includes its original public Sigstore provenance bundle in
`providers/`. CI verifies the archive digest, release workflow, tag, source commit
and GitHub-hosted runner with `gh attestation verify --bundle`. The signing
repository remains the identity recorded when the release was issued; verification
does not depend on GitHub retaining its attestation API entry after a transfer.

Screen integration is being qualified separately from the exported read-only API.
`src/screen.rs` consumes the core-owned `screen` contract; it exposes no input or
clipboard operations. `scripts/check-screen.py --help` describes the explicit
core/provider inputs and private output directory. It verifies the desktop source
and legacy provider, then runs real isolated X11 capture/QUIC/decode, cancellation,
expiry, revocation and IPC compatibility checks under an owned Xvfb server.
The qualification lock pins its dependency graph. A changed core package requires
coordinated lock regeneration and qualification. This is not yet an integrated
NDS viewer or native platform acceptance beyond Linux X11.

Capability discovery reads an explicitly authorized Info-scoped native session;
the view grant remains DesktopView-only. Discovery preserves selection and cannot
claim Watching. The adapter uses shared core module errors/observations, rejects
stale frame access and terminates when an owned source stops producing frames.
Acceptance now owns its X server lifetime and can pause that exact server to
exercise source loss without touching an ambient desktop.

The native profile fits both the 1920-pixel width and 1080-pixel height bounds.
Acceptance includes a real 5120×1440 source and an unchanged image lasting eight
seconds, with the existing frame budget and native heartbeat.

`scripts/check-browser.py --help` describes a separate Linux Chrome qualification:
owned X11 capture, the existing encrypted Iroh/RDS protocol through an owned blind
relay, WebCodecs H264 decoding, canvas rendering and actual compositor pixels.
It checks cancellation, expiry, retained packets, one-use endpoint signatures and
bounded binary/text WebSocket queues. The checked transport overlay caps its Rust
receive queue at eight messages of at most 1 MiB; browser-owned buffers have their
own lifetime. The loopback HTTP/WS fixture is not production HTTPS/WSS acceptance.
Its ephemeral keys and fixture grants do not establish NDS device authorization
or native trust. This code is a qualification input, not an exported NDS browser
viewer. Other browsers, Wayland and other native platforms remain unqualified.
