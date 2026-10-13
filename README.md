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

The Linux library bootstrap is build tooling, separate from screen activation.
`scripts/prepare-library-provider.py --destination CACHE --config CONFIG` reuses
the checked source preparer and emits one consumer-root Cargo override file.
An optional `--archive` accepts the exact published provider source archive.
Generated inputs/config stay outside this repository. Consumers supply CONFIG
to every top-level Cargo command; dependency-local patches are ignored by Cargo.
Use a dedicated Cargo home with this generated config for external Cargo tools
as well; the gate rejects tools that silently alter its lockfile/source graph.
Only `rds-agent/desktop` and `rds-desktop/x11` are selected; the provider GUI,
portal and owned transport backend are excluded from this first library gate.

`scripts/check-library.py --help` describes the downstream archive/public-Git
gate. It requires a new output directory, starts with empty controlled Cargo and
build caches, checks actual source paths/features, dependency policy/licenses and
compiled artifacts. The source archive SHA is explicit. `--resolve-lock` is only
for a reviewed initial lock adoption or a newly published NDS source pin; retain
the resulting Cargo lock for review. Public Git mode uses an explicit
`--consumer-lock PATH` owned by the downstream consumer, independently of the
immutable SDK commit. After one reviewed resolve, repeat the same SDK pin in a
new cache with that external lock and without `--resolve-lock`. The receipt
records both the tooling hash and the exact consumed Git source; committing a
lock that refers to its containing SDK commit would create a re-pin cycle.
Local archive evidence is distinct from the
public Git-consumer gate after source publication. Neither proves NDS endpoint
authorization, host/credential activation or integrated Flutter screen runtime.
