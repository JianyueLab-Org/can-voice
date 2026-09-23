# Voice transport and runtime hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound unauthenticated and protocol input, preserve session correctness, harden simulator/audio/TTS runtime paths, and correct the deployment variable without changing normal voice behavior.

**Architecture:** Keep limits at the ownership boundary: QUIC admission in `transport`, FSD framing in `can-voice-fsd`, subscription correlation in `can-voice-client`, router bookkeeping in `server/internal/router`, and simulator/audio/TTS validation in their existing crates. Each fix adds a focused regression test before the implementation and keeps existing public protocol shapes unless the spec requires an internal option or counter.

**Tech Stack:** Rust workspace (Tokio, CPAL, QUIC client, X-Plane UDP, runtime-loaded SimConnect), Go server (quic-go, router, race detector), Docker Compose, Bun frontend builds.

**Spec:** `can-voice/docs/superpowers/specs/2026-09-23-voice-hardening-design.md`

## Global Constraints

- Preserve normal HELLO, SUB/SUBACK, FSD packet, simulator snapshot, audio playback, and ATIS station behavior.
- Reject oversized or malformed input before allocating or dereferencing variable-length data.
- All caps and timeouts are finite, configurable where the spec calls for configuration, and covered by deterministic tests.
- Do not retain peer identifiers for aggregate admission rejections.
- Do not use `/tmp`; test fixtures and temporary files belong under the project `.temp/` directory and are removed after tests.
- Keep the deployment variable spelling `CAN_VOICE_FSD_FEED` everywhere.

### Task 1: Bound pre-authenticated QUIC connections

**Files:**
- Modify: `server/internal/transport/server.go` (`Config`, `accept`, `serve`)
- Modify: `server/internal/transport/conn.go` (handshake admission release)
- Test: `server/internal/transport/admission_test.go`
- Test: `server/internal/transport/lifecycle_test.go`

**Interfaces:**
- Add `Config.MaxPendingHandshakes int`; zero selects `defaultMaxPendingHandshakes` (128).
- Add `AdmissionStats` with `RejectedPending() uint64` and an internal `admissionGate` with `tryAcquire() bool`, `release()`, and `rejected() uint64`; the counter is aggregate only (`atomic.Uint64`). `Config.AdmissionStats` may be supplied by the caller, otherwise the server owns an internal stats value.
- `accept` calls `tryAcquire` immediately after `Accept`; rejected connections receive `CloseEvicted` and are not passed to `handleConn`.
- `handleConn` defers `gate.release()` until authentication succeeds or fails, including `AcceptStream`, deadline, and malformed HELLO paths.

- [ ] **Step 1: Write failing tests.** Add a gate unit test that accepts exactly the configured number, rejects the next connection, increments the counter, and does not retain peer addresses. Add an accept-loop test with a blocked first handshake proving a second connection is refused before a worker starts; keep the existing handshake deadline test proving an admitted connection still times out.
- [ ] **Step 2: Run tests to verify failure.** Run `go test ./server/internal/transport -run 'TestAdmission|TestHandshake' -count=1`; expect missing gate/config behavior.
- [ ] **Step 3: Implement the gate.** Instantiate one gate per `serve` call, pass it to `accept`/`handleConn`, close rejected QUIC connections without starting per-connection workers, and expose only the aggregate count through an internal test accessor.
- [ ] **Step 4: Run tests to verify success.** Run the focused package tests and `go test -race ./server/internal/transport`.
- [ ] **Step 5: Commit.** `git add server/internal/transport && git commit -m "bound pending voice handshakes"`

### Task 2: Enforce bounded FSD lines and METAR waiter lifetime

**Files:**
- Modify: `crates/can-voice-fsd/src/session.rs` (`attach`, `await_login`, `pump`, METAR request handling)
- Modify: `crates/can-voice-fsd/src/client.rs` only if the timeout API needs an explicit cancellation handle
- Test: `crates/can-voice-fsd/tests/session.rs`

**Interfaces:**
- Define `pub const MAX_LINE_BYTES: usize = 16 * 1024` in `session.rs` and add a `Reason::Protocol(String)` variant for framing violations.
- Replace `BufReader::lines()` with a bounded line reader that consumes `fill_buf()` chunks (or a `take(MAX_LINE_BYTES + 1)` reader) and stops at the first byte beyond the limit; never grow a buffer beyond `MAX_LINE_BYTES` plus the delimiter.
- Store METAR waiters as entries carrying a unique request id and deadline, or remove the exact timed-out sender from `metar_waiters`; cap outstanding requests with `MAX_METAR_WAITERS = 32` and immediately resolve excess requests to `None`.
- Preserve reconnect behavior and resolve all remaining waiters on disconnect/server error.

- [ ] **Step 1: Write failing tests.** Add an integration test sending a line of `MAX_LINE_BYTES + 1` bytes and assert the session closes without accepting the packet. Add a timeout test that issues a METAR request, advances past its timeout while FSD remains connected, then asserts the waiter map is empty and a later report does not resolve the expired request. Add a cap test for 33 concurrent requests.
- [ ] **Step 2: Run tests to verify failure.** Run `cargo test -p can-voice-fsd --test session`; expect the current unbounded reader and retained waiter to fail.
- [ ] **Step 3: Implement bounded framing and cleanup.** Centralize line decoding so login and steady-state paths share the same limit; add deadline-aware waiter removal and the fixed cap.
- [ ] **Step 4: Run tests to verify success.** Run `cargo test -p can-voice-fsd` (loopback tests may require the normal non-sandbox environment).
- [ ] **Step 5: Commit.** `git add crates/can-voice-fsd && git commit -m "bound FSD lines and METAR waiters"`

### Task 3: Serialize SUB messages and correlate SUBACKs

**Files:**
- Modify: `crates/can-voice-client/src/session.rs` (`SubscriptionState`)
- Modify: `crates/can-voice-client/src/pump.rs` (send/ack transitions)
- Test: `crates/can-voice-client/src/session.rs` unit tests
- Test: `crates/can-voice-client/tests/e2e.rs` if a wire-level ordering test is needed

**Interfaces:**
- Keep `SubscriptionState::declare`, `take_pending`, `on_connected`, `on_disconnected`, `on_ack`, `acknowledged`, `denied_rx`, and `denied_tx` public.
- `take_pending` must return `None` while `in_flight.is_some()`; `declare` updates only `desired`/`dirty` while an ACK is outstanding.
- `on_ack` consumes the matching `in_flight`, stores that ACK as acknowledged, and leaves `dirty = true` when `desired != confirmed declaration`.
- Add a connection generation/epoch incremented by `on_connected`; ignore ACKs tagged with an older epoch. `pump.rs` must pass the current epoch to the ACK handler.

- [ ] **Step 1: Write failing tests.** Add tests proving a second declaration cannot produce a second SUB before the first ACK; a delayed first ACK computes denial against the first declaration, not the latest desired declaration; reconnect clears in-flight/acked state; and an old-epoch ACK cannot update the new connection.
- [ ] **Step 2: Run tests to verify failure.** Run `cargo test -p can-voice-client session::tests`; current `in_flight` overwrite should fail the correlation assertions.
- [ ] **Step 3: Implement the state machine.** Gate `take_pending`, retain the latest desired declaration, correlate each ACK with the declaration returned by the preceding `take_pending`, and reset all pending/ack state on disconnect.
- [ ] **Step 4: Run tests to verify success.** Run `cargo test -p can-voice-client` and the existing client e2e tests.
- [ ] **Step 5: Commit.** `git add crates/can-voice-client/src/session.rs crates/can-voice-client/src/pump.rs crates/can-voice-client/tests && git commit -m "correlate subscription acknowledgements"`

### Task 4: Repair router announcement and pilot bookkeeping

**Files:**
- Modify: `server/internal/router/router.go` (`noteTalker`, `removeLocked`)
- Modify: `server/internal/router/fanout.go` only if the announcement call site needs a live-session check
- Modify: `crates/can-voice-fsd/src/pilot_client.rs` (remove `PilotRole::seen` and associated writes/removals)
- Test: `server/internal/router/fanout_test.go` or `router_test.go`
- Test: `crates/can-voice-fsd/src/pilot_client.rs` unit tests if existing role construction references `seen`

**Interfaces:**
- `noteTalker(listener, speaker SessionID) bool` returns false unless `listener` still exists in `r.sessions`; it must not create `announced[listener]` for a removed listener.
- `removeLocked` continues deleting both the removed listener key and removed speaker ids from all announcement sets.
- `PilotRole` retains traffic events and current protocol behavior but no unused callsign-to-time map.

- [ ] **Step 1: Write failing tests.** Add a router test that removes a listener, calls the announcement path, and asserts no `announced` entry is recreated; keep a live-listener positive case and duplicate suppression case. The unused pilot `seen` field is a dead-state removal and needs the existing pilot tests to stay green.
- [ ] **Step 2: Run tests to verify failure.** Run `go test ./server/internal/router -run 'Talker|Announcement'` and `cargo test -p can-voice-fsd pilot_client`; the removed-listener test should expose the current insertion behavior.
- [ ] **Step 3: Implement the live-session guard and delete dead map state.** Check `sessions` under the router lock before allocating the announcement set; remove the field, imports, and updates from `PilotRole`.
- [ ] **Step 4: Run tests to verify success.** Run focused Go and Rust tests.
- [ ] **Step 5: Commit.** `git add server/internal/router crates/can-voice-fsd/src/pilot_client.rs && git commit -m "bound router announcements and remove dead pilot state"`

### Task 5: Authenticate X-Plane RREF sources

**Files:**
- Modify: `crates/can-voice-sim/src/xplane.rs` (`Wire`, `Peer`, `subscribe_and_receive`)
- Test: `crates/can-voice-sim/src/xplane.rs` `link_tests`

**Interfaces:**
- Extend `Wire::recv` to return `(usize, SocketAddr)` or add `recv_from`; `Peer` uses `UdpSocket::recv_from`.
- `subscribe_and_receive` accepts data only when the source equals the selected simulator `SocketAddr`; mismatched packets are ignored without updating `LinkState.values`, `connected`, or liveness timestamps.
- Preserve the existing selected address in `LinkState.address` and return behavior.

- [ ] **Step 1: Write failing tests.** Add a fake wire that emits a valid RREF packet from an unexpected address followed by one from the selected address; assert only the selected packet updates state and establishes `ever`.
- [ ] **Step 2: Run tests to verify failure.** Run `cargo test -p can-voice-sim xplane::link_tests`; the current trait has no source validation and will accept both.
- [ ] **Step 3: Implement source-aware receive.** Thread source addresses through the test wire and production `recv_from`, dropping mismatches before `parse_values`.
- [ ] **Step 4: Run tests to verify success.** Run `cargo test -p can-voice-sim`.
- [ ] **Step 5: Commit.** `git add crates/can-voice-sim/src/xplane.rs && git commit -m "accept X-Plane data only from selected peer"`

### Task 6: Validate SimConnect dispatch lengths before unsafe reads

**Files:**
- Modify: `crates/can-voice-sim/src/msfs.rs` (`SimVarSource::poll`, `SimConnectTraffic::pump`)
- Test: `crates/can-voice-sim/src/msfs.rs` platform-independent dispatch helpers/tests

**Interfaces:**
- Add internal helpers `dispatch_has_bytes(size: c_ulong, required: usize) -> bool` and `dispatch_payload<T>(data: *const Recv, size: c_ulong) -> Option<&T>` (or an equivalent checked slice API) that validate `size >= required` and use checked arithmetic for variable payloads.
- For `RecvSimObjectData`, require the fixed header and `define_count * size_of::<c_double>()` bytes before reading values; reject count overflow and truncated packets.
- For assigned-object and exception packets, require `size_of::<RecvAssignedObjectId>()` / `size_of::<RecvException>()` before field access; log one bounded diagnostic per dropped packet class.

- [ ] **Step 1: Write failing tests.** Add helper tests for exact-size acceptance, one-byte truncation rejection, count overflow rejection, and valid variable payload decoding. Feed truncated assigned/exception fixtures to the traffic decoder and assert no output/no panic.
- [ ] **Step 2: Run tests to verify failure.** Run `cargo test -p can-voice-sim msfs`; current code dereferences the structs without consulting `size`.
- [ ] **Step 3: Implement checked dispatch decoding.** Check the reported byte length before every cast or `add`, then decode only the validated payload range.
- [ ] **Step 4: Run tests to verify success.** Run `cargo test -p can-voice-sim` on all platforms; Windows CI additionally exercises the runtime-loaded API path.
- [ ] **Step 5: Commit.** `git add crates/can-voice-sim/src/msfs.rs && git commit -m "validate SimConnect dispatch lengths"`

### Task 7: Reject stale jitter tails

**Files:**
- Modify: `crates/can-voice-client/src/rx/jitter.rs` (`JitterBuffer::push`)
- Test: `crates/can-voice-client/src/rx/jitter.rs` tests

**Interfaces:**
- In `push`, expand the sequence first and reject packets older than the reorder/playback window before mutating `last`, `high`, `frames`, or sequence state.
- Keep accepted duplicate, late, and 16-bit wrap semantics unchanged; a `last` flag from an earlier talkspurt must not finish the current buffer.

- [ ] **Step 1: Write failing tests.** Add a stale-tail test that pushes a new talkspurt, then an old `last` packet, and asserts playback does not return `End`; add boundary tests for the reorder window, duplicate frames, and wrapped sequence numbers.
- [ ] **Step 2: Run tests to verify failure.** Run `cargo test -p can-voice-client jitter`; the current code updates `last` before stale rejection.
- [ ] **Step 3: Move stale rejection ahead of tail/high mutation.** Use the existing expanded sequence and window constants; do not change normal late/duplicate/wrap behavior.
- [ ] **Step 4: Run tests to verify success.** Run the complete client test suite.
- [ ] **Step 5: Commit.** `git add crates/can-voice-client/src/rx/jitter.rs && git commit -m "ignore stale jitter tails"`

### Task 8: Remove real-time audio callback allocations

**Files:**
- Modify: `crates/can-voice-client/src/audio.rs` (`AudioIo::start`, output/input callbacks, `fill_output`)
- Test: `crates/can-voice-client/src/audio.rs` tests

**Interfaces:**
- Prepare callback scratch buffers before starting CPAL streams, sized to the selected device buffer; capture them in the callback without allocating.
- Add allocation-free conversion helpers for F32↔I16 that write directly into the supplied destination or reusable scratch slice.
- Preserve channel folding, playback clock steering, silence fill, and output layout.

- [ ] **Step 1: Write failing tests.** Extract a callback-sized conversion function and exercise F32 and I16 output with an allocation-counting test allocator. Assert zero allocations after setup, plus the existing sample and channel-layout expectations. The failure must come from the actual conversion path used by CPAL, not a source-text assertion.
- [ ] **Step 2: Run tests to verify failure.** Run `cargo test -p can-voice-client audio`; current F32 callbacks allocate `Vec` on every invocation.
- [ ] **Step 3: Implement preallocated scratch conversion.** Allocate once during stream setup, pass mutable scratch into conversion, and remove `Vec::with_capacity`, `vec!`, and iterator `collect` from CPAL callbacks.
- [ ] **Step 4: Run tests to verify success.** Run `cargo test -p can-voice-client`; run the platform audio smoke test where a device is available.
- [ ] **Step 5: Commit.** `git add crates/can-voice-client/src/audio.rs && git commit -m "remove audio callback allocations"`

### Task 9: Bound ATIS TTS input/output and cancel child processes

**Files:**
- Modify: `crates/can-voice-atis/src/tts.rs` (`CommandTts`, `run`, PCM decode)
- Modify: `crates/can-voice-atis/src/vatis.rs`/`station.rs` only where feed entries enter `speak`
- Test: `crates/can-voice-atis/src/tts.rs` tests
- Test: `crates/can-voice-atis/src/vatis.rs` or station tests for per-station isolation

**Interfaces:**
- Define explicit caps: `MAX_TTS_TEXT_BYTES = 16 * 1024`, `MAX_MEDIA_BYTES = 16 * 1024 * 1024`, `MAX_PCM_SAMPLES = 48_000 * 120` (two minutes).
- `CommandTts::speak` rejects oversized UTF-8 text before spawning; `run` uses `tokio::process::Child` with `kill_on_drop(true)`, bounded file/output reads, and a finite `TTS_TIMEOUT` (120 seconds).
- On timeout, terminate and reap the child; on future cancellation, `kill_on_drop` terminates it. Remove the temporary media file in a cancellation-safe guard on success, error, and drop.
- Invalid/oversized feed entries return an entry error to the station loop; the loop logs and continues with other stations.

- [ ] **Step 1: Write failing tests.** Add tests for text-byte rejection without process spawn, oversized decoded PCM rejection, timeout termination of a sleeping fake command, cancellation cleanup of the media file, and one invalid station entry not aborting a second valid station.
- [ ] **Step 2: Run tests to verify failure.** Run `cargo test -p can-voice-atis tts`; current code has no size/time bounds and uses uncancellable `.status()`/`.output()` processes.
- [ ] **Step 3: Implement bounded process and media lifecycle.** Add constants, bounded file/output reads, timeout/cancellation cleanup, and per-entry error isolation. Use `.temp/` fixtures for fake commands and remove them with test cleanup.
- [ ] **Step 4: Run tests to verify success.** Run `cargo test -p can-voice-atis` and the ATIS golden tests.
- [ ] **Step 5: Commit.** `git add crates/can-voice-atis/src/tts.rs crates/can-voice-atis/src/vatis.rs crates/can-voice-atis/src/station.rs && git commit -m "bound and cancel ATIS TTS work"`

### Task 10: Correct the deployment variable documentation

**Files:**
- Modify: `docs/deploy.md` line 78 and surrounding environment-variable table

**Interfaces:**
- Every deployment instruction and example names `CAN_VOICE_FSD_FEED`; do not introduce or preserve the stale `CAN_FSD_FEED` spelling.
- Runtime configuration remains `LoadConfig(get)` reading `CAN_VOICE_FSD_FEED` with the existing default URL.

- [ ] **Step 1: Confirm the mismatch.** Run `rg -n 'CAN_FSD_FEED|CAN_VOICE_FSD_FEED' docs/deploy.md server/docker-compose.yml server/cmd/can-voice`; the guide currently differs from Compose and runtime configuration.
- [ ] **Step 2: Fix the documentation.** Replace the stale deployment spelling with `CAN_VOICE_FSD_FEED`.
- [ ] **Step 3: Verify the contract.** Run the same bounded search and confirm the guide, Compose, and runtime configuration use the same name.
- [ ] **Step 4: Commit.** `git add docs/deploy.md && git commit -m "fix FSD feed deployment variable"`

### Task 11: Run the complete verification matrix

**Files:**
- Modify: none unless a test exposes an integration mismatch
- Test: repository CI workflows and release scripts

**Interfaces:**
- Rust verification includes workspace tests and clippy.
- Go verification includes race tests and vet.
- Frontend verification covers all four app builds.
- Release verification covers the existing release-script tests.

- [ ] **Step 1: Run Rust checks.** From `can-voice/`, run `cargo test --workspace` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- [ ] **Step 2: Run Go checks.** Run `go test -race ./...` and `go vet ./...` from `can-voice/server` (or the repository Go module root).
- [ ] **Step 3: Run frontend checks.** Run the repository’s Bun build command for `apps/xpc`, `apps/msfs`, `apps/atis`, and `apps/controller`.
- [ ] **Step 4: Run release checks.** Run the existing release workflow/script tests and the deployment variable grep.
- [ ] **Step 5: Record environment limits.** Report any loopback-bind, SimConnect-DLL, or audio-device tests that cannot run in CI; retain their deterministic source/helper tests and do not mark them as passing without output.

## Self-review

- Spec coverage: all eleven reviewed findings are assigned to Tasks 1–10; Task 11 covers the spec’s final verification requirements.
- No production code is changed by this plan.
- The plan preserves protocol compatibility and makes new limits internal/configuration-only except for `Config.MaxPendingHandshakes`.
- The deployment typo is intentionally limited to documentation; runtime already reads `CAN_VOICE_FSD_FEED`.
