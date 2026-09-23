# Voice Identity and Radio Authority Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace client-controlled station and observer authority with signed, short-lived voice grants enforced by can-api, can-voice, and every voice client while preserving pilot/observer TX under the approved role contract.

**Architecture:** can-api remains the only token issuer. It signs role, controller callsign or ATIS-only station scope, bounded TX frequencies, and expiry after checking the live FSD feed and configured ATIS ownership. can-voice verifies the claim locally, binds HELLO station identity only for ATIS sessions, intersects every SUB with the claim and live assignment, and removes TX/XC when authority expires or disappears. Clients retain declarative desired radio state and reconnect with refreshed tickets; UI/feed checks remain advisory.

**Tech Stack:** Go (`can-api`, `can-voice/server`), Rust (`can-voice` crates and Tauri apps), Ed25519 signed tokens, QUIC control/datagram protocol, existing FSD datafeed and client tests.

**Spec:** `can-voice/docs/superpowers/specs/2026-09-23-voice-identity-design.md`

## Global Constraints

- Keep the token response shape exactly `{token, expires_in}`.
- Never log passwords or signed tickets.
- Ticket roles are exactly `pilot`, `observer`, `controller`, and `atis`.
- Ordinary pilot/observer sessions have empty station scope; ATIS station scope is allowed only for the configured fleet CAN ID and configured callsign.
- Pilot and observer TX is one in-band COM frequency at a time and requires the same CID's matching FSD role to be online.
- Controller TX is limited to a separately signed online callsign and its assigned primary frequency; controller station scope remains empty unless the role is ATIS.
- A controller's client-requested extra frequency is never signed as TX authority; it is RX-only unless a separate server-owned assignment/allowlist is added.
- Pilot/observer authority defaults to at most one requested in-band COM frequency because the current FSD feed proves online role presence but does not expose COM1; no second pilot/observer TX frequency or XC pair is accepted.
- RX remains subject to the existing count limit; TX and XC are authoritative server decisions.
- Position-feed degradation may fail open for RX only. It must never grant TX.
- The decoder may accept an empty legacy `follow` during migration; nonempty `follow` is rejected after the clients stop emitting it.
- Rollout order is issuer and client request support, then strict server enforcement.

---

### Task 1: Define the signed voice grant and issuer inputs

**Files:**
- Modify: `can-api/internal/voiceauth/voiceauth.go`
- Modify: `can-api/internal/api/voicetoken.go`
- Modify: `can-api/internal/config/config.go`
- Modify: `can-api/internal/fsd/datafeed.go`
- Modify: `can-api/data/atis/config.json` loading/validation path used by `can-api/internal/api/atis.go`
- Test: `can-api/internal/voiceauth/voiceauth_test.go`
- Test: `can-api/internal/api/voicetoken_test.go`
- Test: `can-api/internal/fsd/datafeed_test.go`

**Interfaces:**
- Consume: `voiceauth.Signer`, `fsd.DatafeedReader.Online`, configured ATIS station document, and existing `voiceTokenRequest` credentials.
- Produce: signed claim fields `cid`, `rating`, `role`, optional ATIS-only `station`, optional controller `callsign`, bounded `tx`, and `exp`; a role-aware internal issuer method while retaining the external response `{token, expires_in}`.

- [ ] **Step 1: Write failing claim and policy tests.**

  Add tests that decode an issued claim and assert exact role/callsign/station/TX equality for:

  ```go
  Claims{CID: "1000", Rating: 5, Role: "controller", Callsign: "ZSPD_TWR", Station: "", TX: []int{118500}, Exp: goldenExp}
  ```

  Add table tests for:

  - pilot with one selected in-band COM frequency while its FSD role is online;
  - observer with one online facility-0 entry and no station scope;
  - controller with its online callsign and assigned frequency;
  - ATIS fleet CID with one configured callsign/frequency;
  - wrong CID, offline callsign, unowned ATIS station, and unowned controller/ATIS frequency returning a refusal;
  - empty or out-of-band frequency never entering `tx`.

- [ ] **Step 2: Run the focused tests and verify they fail.**

  Run:

  ```bash
  go test ./internal/voiceauth ./internal/api ./internal/fsd
  ```

  Expected: compile failures for the new claim fields and issuer policy helpers, plus failing policy assertions.

- [ ] **Step 3: Extend the wire claim without changing the HTTP reply.**

  Add `Role string`, `Station string`, and `TX []int` (or the repository's established integer width) to `voiceauth.Claims` with stable JSON names. Preserve the existing field order for the old fields, update the cross-repository golden token, and reject unsupported roles, duplicate TX frequencies, and TX outside `118000..136975` before signing.

- [ ] **Step 4: Add issuer policy evaluation.**

  In the API layer, after credential/rating checks, load one bounded FSD snapshot and configured ATIS stations. Extend `can-api/internal/fsd.OnlineSnapshot` to retain ATIS and facility-0 observer entries (the current reader exposes only pilots/controllers), including the frequency when the upstream feed supplies one. Derive the role in this order: configured ATIS ownership, controller assignment, observer facility `0`, pilot presence. A controller ticket signs `Callsign` and the feed's assigned primary frequency; `Station` stays empty. A client-requested extra frequency is ignored and remains RX-only. For pilot/observer roles, the request may select one in-band COM frequency because FSD proves online role presence but does not expose COM1 in the current feed; reject a second requested TX frequency. This is a role-limited frequency choice, not proof of FSD-reported COM1. Do not infer controller or ATIS authority from a client-supplied station or frequency. Return the existing JSON error envelope/status for refused scope requests and issue an empty-TX token when the account is valid but currently RX-only.

- [ ] **Step 5: Run the focused tests and verify they pass.**

  Run:

  ```bash
  go test ./internal/voiceauth ./internal/api ./internal/fsd
  ```

  Expected: PASS, including exact claim equality and unchanged `{token, expires_in}` shape tests.

---

### Task 2: Make TokenSource request scoped tickets and renew them

**Files:**
- Modify: `can-voice/crates/can-voice-token/src/lib.rs`
- Modify: `can-voice/crates/can-voice-app/src/bridge.rs`
- Test: `can-voice/crates/can-voice-token/src/lib.rs`
- Test: `can-voice/crates/can-voice-app/src/bridge.rs`

**Interfaces:**
- Consume: `can_voice_client::Config` (`follow`, `station`), client role/callsign/frequency context, and can-api's unchanged token response. Pilot/observer callers must establish their FSD session and wait for its Online event before calling the voice-token endpoint; controller/ATIS callers must likewise wait for their authoritative online feed entry.
- Produce: `TokenRequest { cid, password, role, callsign, station, frequency }` (or equivalent typed request) and `TokenSource::fetch_for(scope)`; `Bridge::connect` and renewal must use the same immutable scope and fetch a fresh ticket.

- [ ] **Step 1: Write failing request-shape and secrecy tests.**

  Assert that the HTTP body includes role/station/frequency scope, the response parser still accepts only `{token, expires_in}`, and `Debug` output for `TokenSource`/scope contains neither password nor token.

- [ ] **Step 2: Run the focused Rust tests and verify they fail.**

  ```bash
  cargo test -p can-voice-token -p can-voice-app
  ```

- [ ] **Step 3: Implement typed scope and renewal.**

  Keep credentials private inside `TokenSource`; add a cloneable scope object. Make `can_voice_token::connect` fetch with the scope before each new `VoiceClient::connect`, and make `bridge::renew` reuse the saved scope rather than reconstructing it from mutable UI state. Do not let a voice-first startup path request a pilot/observer token: the FSD handle must report Online first, and any later voice-connect failure must tear down that FSD handle.

- [ ] **Step 4: Run the focused Rust tests and verify they pass.**

  ```bash
  cargo test -p can-voice-token -p can-voice-app
  ```

---

### Task 3: Verify claims at HELLO and carry authority in the server session

**Files:**
- Modify: `can-voice/server/internal/auth/token.go`
- Modify: `can-voice/server/internal/control/message.go`
- Modify: `can-voice/server/internal/router/session.go`
- Modify: `can-voice/server/internal/router/router.go`
- Modify: `can-voice/server/internal/transport/conn.go`
- Test: `can-voice/server/internal/auth/token_test.go`
- Test: `can-voice/server/internal/transport/handshake_test.go`
- Test: `can-voice/server/internal/router/router_test.go`

**Interfaces:**
- Consume: signed claim from Task 1 and `control.Hello.Station`.
- Produce: `router.Session` authority fields (`Role`, canonical `Callsign`, ATIS-only `Station`, `TXGrant`, grant expiry/assignment identity) and a handshake rejection when HELLO station is nonempty for a non-ATIS claim or differs from the signed ATIS station.

- [ ] **Step 1: Write failing handshake and claim-equality tests.**

  Cover:

  - signed controller claim `Callsign=ZSPD_TWR`, empty station, and HELLO empty station succeeds;
  - signed ATIS claim `Station=ZSPD_ATIS` plus HELLO `station=ZSPD_ATIS` succeeds;
  - HELLO `ZBAA_TWR` with the same ticket is refused;
  - ATIS station scope allows `(CID, station)` multiplicity only for the signed station;
  - pilot/observer tickets reject nonempty HELLO station;
  - legacy empty station remains valid for ordinary sessions during rollout.

- [ ] **Step 2: Run the focused Go tests and verify they fail.**

  ```bash
  go test ./internal/auth ./internal/transport ./internal/router
  ```

- [ ] **Step 3: Extend `auth.Claims`, `router.SessionOpts`, and handshake mapping.**

  Verify the Ed25519 claim before trusting role/station/TX. Canonicalize and validate callsigns once. Use the signed claim, not HELLO, for role and TX authority. Keep `Hello.Follow` accepted only when empty after migration; reject nonempty values in the strict server path.

- [ ] **Step 4: Run the focused Go tests and verify they pass.**

  ```bash
  go test ./internal/auth ./internal/transport ./internal/router
  ```

---

### Task 4: Enforce signed TX grants, XC, expiry, and live controller assignment

**Files:**
- Modify: `can-voice/server/internal/router/router.go`
- Modify: `can-voice/server/internal/router/fanout.go`
- Modify: `can-voice/server/internal/transport/conn.go`
- Modify: `can-voice/server/internal/fsdfeed/feed.go`
- Modify: server startup wiring that currently calls `Router.SetLocator`
- Test: `can-voice/server/internal/router/router_test.go`
- Test: `can-voice/server/internal/router/fanout_test.go`
- Test: `can-voice/server/internal/transport/uplink_test.go`

**Interfaces:**
- Consume: `Session.TXGrant`, `fsdfeed.Locator.Positions`, and existing `Router.Subscribe`, `MayTransmit`, `Fanout`.
- Produce: SUBACK rejection for unauthorized TX/XC, no TX after grant expiry or assignment loss, RX continuity when TX disappears, and an explicit authority-loss event/notification for clients.

- [ ] **Step 1: Write failing authorization tests.**

  Add tests that prove:

  - a pilot/observer can declare one matching COM TX but a second TX is rejected;
  - a controller can TX only its signed assigned frequency;
  - a controller's XC pair is accepted only when both frequencies are in the signed grant and live assignment permits them;
  - a forged client declaration cannot add TX authority;
  - feed loss/staleness denies new TX and removes existing TX/XC while preserving RX;
  - token expiry removes TX/XC and permits an RX-only session to remain connected;
  - handoff removes the old controller's TX before accepting the new assignment.

- [ ] **Step 2: Run focused tests and verify they fail.**

  ```bash
  go test ./internal/router ./internal/transport
  ```

- [ ] **Step 3: Intersect declarations with signed authority.**

  In `subscribeLocked`, build the accepted TX set from the declared TX list intersected with the session grant, then derive `TX ⊆ RX` and normalize XC only over that accepted set. Preserve the existing RX count and bounded rejection reporting. `MayTransmit` must check the accepted set, not just the raw declaration.

- [ ] **Step 4: Add live authority reconciliation.**

  Extend the locator/update path so controller/pilot/observer assignment is checked on every transmit decision and after feed updates. Add a timer/deadline derived from the signed `exp`; when it fires, atomically remove TX/XC authorization and leave RX subscriptions intact. On unavailable/stale authority data, stop TX/XC and deny new TX; do not apply this fail-closed rule to RX range delivery. A refreshed connection must receive a new ticket before the old deadline and replay its latest SUB.

- [ ] **Step 5: Run focused tests and verify they pass.**

  ```bash
  go test ./internal/router ./internal/transport
  ```

---

### Task 5: Update protocol/client state for scoped authority and SUBACK truth

**Files:**
- Modify: `can-voice/crates/can-voice-proto/src/control.rs`
- Modify: `can-voice/crates/can-voice-client/src/conn.rs`
- Modify: `can-voice/crates/can-voice-client/src/session.rs`
- Modify: `can-voice/crates/can-voice-client/src/pump.rs`
- Modify: `can-voice/crates/can-voice-client/src/client.rs`
- Test: existing protocol golden tests and client connection/session tests

**Interfaces:**
- Consume: server SUBACK/NOTICE authority-loss messages and Task 2 scoped ticket renewal.
- Produce: client `Event::TxDenied`, `Event::RxDenied`, `Event::XcDenied`, and `Event::Notice` updates that reflect effective server state; reconnect resends the latest desired SUB with the refreshed scope.

- [ ] **Step 1: Write failing client tests.**

  Add tests for an authority-loss notice, scope-aware reconnect, empty legacy `follow`, strict rejection of nonempty `follow`, and SUBACK where RX survives while TX/XC are rejected.

- [ ] **Step 2: Run focused tests and verify they fail.**

  ```bash
  cargo test -p can-voice-proto -p can-voice-client
  ```

- [ ] **Step 3: Implement only additive protocol fields/events.**

  Keep `Hello.follow` serialization compatible for empty values, stop new clients from populating it, preserve `Hello.station` for ATIS, and make `SubscriptionState` use ACKed TX/RX/XC as effective state after every reconnect.

- [ ] **Step 4: Run focused tests and verify they pass.**

  ```bash
  cargo test -p can-voice-proto -p can-voice-client
  ```

---

### Task 6: Convert controller authority from advisory feed gate to scoped ticket plus effective-state UI

**Files:**
- Modify: `can-voice/apps/controller/src-tauri/src/lib.rs`
- Modify: `can-voice/crates/can-voice-app/src/bridge.rs`
- Modify: `can-voice/crates/can-voice-client/src/stack.rs` only where effective ACK state requires it
- Modify: `can-voice/apps/controller/src/App.vue`
- Test: `can-voice/apps/controller/src-tauri/src/lib.rs` tests
- Test: `can-voice/crates/can-voice-app/src/bridge.rs` tests

**Interfaces:**
- Consume: `can_voice_datafeed::controller_for`, scoped `TokenSource`, server ACKs, and existing `FeedView`.
- Produce: controller connect requests with role/controller callsign/frequency, no client-supplied station authority, advisory feed polling, and UI state derived from effective server grants.

- [ ] **Step 1: Write failing controller tests.**

  Cover controller ticket scope, assignment change requiring a fresh ticket, RX-only additional frequencies, unauthorized TX/XC remaining denied after local state changes, and reconnect preserving desired RX/TX/XC while showing server-effective state.

- [ ] **Step 2: Run focused tests and verify they fail.**

  ```bash
  cargo test -p can-voice-app --manifest-path apps/controller/src-tauri/Cargo.toml
  ```

- [ ] **Step 3: Request controller-scoped tickets.**

  At `connect`, obtain the current `controller_for(cid, feed)` result before voice connect; pass role `controller`, callsign, and assigned primary frequency to `TokenSource`. Keep `feed.transmit_allowed` as UX only. On feed assignment changes, stop/reconnect with a fresh scope and replay the stack. Additional controller frequencies stay RX-only unless a server-owned allowlist is added; the client cannot expand the signed TX set.

- [ ] **Step 4: Keep the shared bridge authoritative for PTT/audio/reconnect.**

  Do not add direct UI-to-server authority paths. `Bridge::set_transmitting`, `set_audio_devices`, `set_volume`, and `supervise/renew` remain the only implementations. Render TX/XC from ACKed/effective state and display denial notices without pretending the local desired state was granted.

- [ ] **Step 5: Run focused tests and verify they pass.**

  ```bash
  cargo test -p can-voice-app --manifest-path apps/controller/src-tauri/Cargo.toml
  ```

---

### Task 7: Make ATIS station scope authoritative and preserve fleet multiplicity

**Files:**
- Modify: `can-voice/crates/can-voice-atis/src/station.rs`
- Modify: `can-voice/crates/can-voice-atis/src/main.rs`
- Modify: `can-voice/apps/atis/src-tauri/src/lib.rs` if desktop ATIS starts voice sessions directly
- Modify: `can-api/internal/api/voicetoken.go` ATIS ownership lookup wiring
- Test: `can-voice/crates/can-voice-atis/src/station.rs`
- Test: `can-api/internal/api/voicetoken_test.go`

**Interfaces:**
- Consume: configured ATIS station list, fleet CAN ID, `TokenSource` scope, and existing `station::voice_config` with `station` callsign.
- Produce: one signed ATIS ticket per configured station, `HELLO.station` equal to the signed station, one TX frequency equal to configured frequency, and no cross-station reuse.

- [ ] **Step 1: Write failing ATIS tests.**

  Add tests proving two configured stations under one fleet CID can connect simultaneously, each ticket names only its own callsign/frequency, an unconfigured callsign is refused, and a station cannot use another station's frequency.

- [ ] **Step 2: Run focused tests and verify they fail.**

  ```bash
  cargo test -p can-voice-atis
  go test ./internal/api
  ```

- [ ] **Step 3: Pass ATIS scope to TokenSource and retain station HELLO.**

  Keep `VoiceSettings.tokens` credential-backed and non-loggable. Build the scope from the configured station, not UI input or datafeed text. Preserve `audio_devices: false`, TTS injection, and the existing one-frequency SUB.

- [ ] **Step 4: Run focused tests and verify they pass.**

  ```bash
  cargo test -p can-voice-atis
  go test ./internal/api
  ```

---

### Task 8: Replace observer follow mode with own FSD presence and own-position range

**Files:**
- Modify: `can-voice/apps/xpc/src-tauri/src/lib.rs`
- Modify: `can-voice/apps/msfs/src-tauri/src/lib.rs`
- Modify: `can-voice/crates/can-voice-app/src/observer.rs`
- Modify: `can-voice/crates/can-voice-datafeed/src/lib.rs`
- Modify: `can-voice/crates/can-voice-fsd/src/pilot.rs`
- Modify: `can-voice/crates/can-voice-fsd/src/pilot_client.rs`
- Modify: `can-api/internal/fsd/datafeed.go` if issuer-side role derivation needs explicit observer entries
- Modify: `can-voice/apps/xpc/src/App.vue`, `can-voice/apps/xpc/src/components/PilotSettings.vue`, `can-voice/apps/xpc/src/types.ts`
- Modify: `can-voice/apps/msfs/src/App.vue`, `can-voice/apps/msfs/src/components/PilotSettings.vue`, `can-voice/apps/msfs/src/types.ts`
- Modify: observer UI/settings components in `can-voice/apps/xpc/src` and `can-voice/apps/msfs/src`
- Test: XPC and MSFS Tauri Rust tests
- Test: `can-voice/crates/can-voice-app/src/observer.rs`

**Interfaces:**
- Consume: simulator snapshot/position, observer callsign field, `PilotIdentity`, `PilotConfig`, and scoped `TokenSource`.
- Produce: observer FSD login with the observer's own CID/callsign and facility `0`, local position publication, voice HELLO with empty `follow`, one in-band COM TX scope while observer FSD presence is online, and explicit no-flight-plan/IDENT/aircraft-injection behavior.

- [ ] **Step 1: Write failing observer coordination tests.**

  Cover own observer callsign validation, observer FSD config with facility `0`, local position propagation, empty `follow` on voice config, manual frequency overriding COM1, missing position withholding geographic TX/range grant, and teardown when any of FSD, voice, or simulator startup fails.

- [ ] **Step 2: Run focused tests and verify they fail.**

  ```bash
  cargo test --manifest-path apps/xpc/src-tauri/Cargo.toml
  cargo test --manifest-path apps/msfs/src-tauri/Cargo.toml
  cargo test -p can-voice-app
  ```

- [ ] **Step 3: Add one owner for coordinated session lifecycle.**

  Start FSD first with the observer's own CAN ID/callsign and wait until the FSD link is Online. Only then request the scoped voice ticket and connect voice, followed by the simulator pump. If a later start fails, stop every resource already started. On disconnect, abort pump, stop FSD, clear observer state, and disconnect voice in that order. Do not borrow another member's `follow` position.

- [ ] **Step 4: Update observer role and UI fields.**

  Replace persisted `follow` as the authority input with an observer callsign. Keep manual observer frequency precedence. Do not expose flight-plan, IDENT, or aircraft-injection capabilities in observer mode; allow text only when the observer FSD role supports it.

- [ ] **Step 5: Preserve observer presence and own-position routing in the datafeed.**

  Keep facility `0` entries in the voice datafeed representation instead of dropping them in `can-voice-datafeed::position_from`. Mark them as observers, index them by CID for authority lookup, and use their own FSD position for voice range. Cap observer range at 100 NM in the server-side position/range model regardless of client-declared visual range. A missing or stale observer position must produce no geographic TX grant; it must not fall back to `HELLO.follow` or another member's coordinates.

- [ ] **Step 6: Run focused tests and verify they pass.**

  ```bash
  cargo test --manifest-path apps/xpc/src-tauri/Cargo.toml
  cargo test --manifest-path apps/msfs/src-tauri/Cargo.toml
  cargo test -p can-voice-app
  cargo test -p can-voice-datafeed
  ```

---

### Task 9: Rollout migration checks and end-to-end verification

**Files:**
- Modify: `can-voice/server/internal/transport/conn.go` rollout gate and deployment configuration
- Modify: `can-api/internal/config/config.go` deployment defaults/documentation
- Modify: `can-voice/apps/controller/src-tauri/src/lib.rs`, `apps/xpc/src-tauri/src/lib.rs`, `apps/msfs/src-tauri/src/lib.rs`, and ATIS client configuration to stop emitting nonempty `follow`
- Test: server handshake, router, transport, token issuer, all client crates/apps
- Verify: `can-voice/docs/manual-test.md` and deployment manifests

**Interfaces:**
- Consume: all scoped-ticket clients from Tasks 2 and 6–8.
- Produce: staged deployment with old clients RX-only, then strict rejection of nonempty `follow` and unauthorized station/TX claims.

- [ ] **Step 1: Add rollout tests.**

  Verify an old/unscoped client can establish RX-only service, cannot obtain TX in SUBACK, and cannot transmit datagrams. Verify a scoped client receives the expected role/frequency grant and renews before expiry.

- [ ] **Step 2: Run the complete test matrix before enabling strict mode.**

  ```bash
  go test ./...
  cargo test --workspace
  bun --cwd apps/controller run build
  bun --cwd apps/xpc run build
  bun --cwd apps/msfs run build
  bun --cwd apps/atis run build
  ```

- [ ] **Step 3: Verify the issuer/client request-support boundary without changing production state.**

  In a test configuration only, provide the Ed25519 public key to can-voice and the existing private seed to can-api. Keep the HTTP reply shape unchanged. Confirm token issuance, renewal, and client scope logging contain no password or token values. Record the required deployment configuration for a later operator-led rollout; do not edit live deployment state in this task.

- [ ] **Step 4: Verify strict-mode gates are ready without enabling production enforcement.**

  Exercise claim-vs-HELLO station checks, live assignment reconciliation, TX expiry cleanup, and rejection of nonempty `follow` in tests and a local fixture. Keep the empty legacy field accepted only for the migration window. Do not switch production rollout flags or deployment manifests here.

- [ ] **Step 5: Execute handoff, feed-loss, observer, ATIS, and teardown scenarios.**

  Confirm controller handoff removes old TX, feed loss leaves RX but stops TX/XC, token renewal restores the latest SUB, ATIS stations coexist under one fleet CID, observer traffic uses the observer's own FSD position, and partial startup failures release every started resource.

## Spec contradictions found

- Current XPC/MSFS observer mode is explicitly voice-only and sends `HELLO.follow` (`apps/xpc/src-tauri/src/lib.rs:699-723`, mirrored in MSFS). The approved spec requires an observer FSD session with the observer's own CAN ID/callsign, facility `0`, and local simulator position. Task 8 intentionally changes this behavior.
- Current can-voice `Hello.Station` is an eviction key only; the server does not authorize it (`server/internal/transport/conn.go:358-390`, `server/internal/router/router.go:105-128`). Tasks 1, 3, and 4 make the signed claim authoritative.
- Current `can-api/internal/voiceauth.Claims` has only `cid`, `rating`, `max_tx`, and `exp`; the current golden token tests pin that shape. Task 1 updates both sides together while retaining the external `{token, expires_in}` response.
- Current controller feed polling already gates local TX, but that gate is advisory and client-controlled (`apps/controller/src-tauri/src/lib.rs:apply_feed`). Task 6 retains it for UX and moves permission to server-verified ticket/assignment state.
- Current `can-voice-atis` already sends station callsigns in HELLO and supports one session per configured station, but the ticket issuer does not yet bind the station/frequency. Task 7 adds that binding.
