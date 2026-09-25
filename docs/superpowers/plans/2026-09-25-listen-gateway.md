# CAN Listen Gateway Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide a receive-only web player at `listen.ceruleanavi.net` for live CAN radio traffic while keeping `audio.ceruleanavi.net:64738/udp` unchanged.

**Architecture:** A web service validates the shared CAN session cookie through `can-api`. A Rust gateway reuses `can-voice-client` to receive Opus frames from `audio.ceruleanavi.net`, decodes and mixes selected frequencies, and exposes a browser-compatible chunked PCM stream. Browser sessions never receive CAN network passwords or transmit grants.

**Tech Stack:** Rust, existing `can-voice-client`, Go `can-api`, HTTP streaming, browser AudioContext, HTTPS.

**Spec:** Approved chat design on 2026-09-25.

## Global Constraints

- `audio.ceruleanavi.net` remains direct QUIC/UDP; do not route it through Cloudflare Tunnel.
- Web authentication uses the parent-domain `can_session` cookie and server-side `/api/v1/auth/session` validation.
- The listener has no transmit capability; receive-only authorization is explicit and tested.
- Do not put CAN passwords, session secrets, or voice signing keys in browser code.
- Initial UI supports one selected frequency; the gateway interface must allow later multi-frequency mixing.

---

### Task 1: Receive-only authorization contract

**Files:**
- Modify: `can-api/internal/voiceauth/voiceauth.go`
- Modify: `can-api/internal/api/voicetoken.go`
- Test: existing voiceauth and voicetoken test files

- [ ] Add a named listener role whose issued claims contain no TX frequencies and are accepted by the verifier.
- [ ] Add an authenticated session-based listener grant endpoint for the gateway; keep the password-based client endpoint unchanged.
- [ ] Add tests proving listener grants cannot request or obtain TX authorization.
- [ ] Run the focused Go tests.

### Task 2: Login callback and session plumbing

**Files:**
- Modify: `can-web/src/lib/callbackUrl.ts`
- Create or modify: the selected `listen` web service session helper
- Test: callback URL and session helper tests

- [ ] Allow only the exact `https://listen.ceruleanavi.net` origin as a callback.
- [ ] Forward `can_session` server-side to `GET /api/v1/auth/session`.
- [ ] Return an unauthenticated state on upstream timeout, non-200, or `{ user: null }`.
- [ ] Add tests for valid, invalid, and unavailable session responses.

### Task 3: Rust receive gateway

**Files:**
- Create: `can-voice/listen-gateway/` binary crate
- Modify: workspace membership and deployment documentation
- Test: gateway unit and protocol tests

- [ ] Connect with `can-voice-client` using a server-only listener grant.
- [ ] Subscribe to one configured frequency and expose decoded 48 kHz mono frames through an internal track interface.
- [ ] Reject all transmit declarations and datagrams in gateway code.
- [ ] Reconnect and restore the receive subscription after a voice link interruption.
- [ ] Add bounded per-listener buffering and disconnect slow browser consumers.
- [ ] Run Rust unit and integration tests.

### Task 4: Browser transport and player

**Files:**
- Create: `listen` web frontend and backend routes
- Create: WebRTC signaling and player modules
- Test: browser-facing route and signaling tests

- [ ] Require a validated CAN session before creating a player session.
- [ ] Request only a short-lived listener session from `can-api` through the gateway.
- [ ] Render frequency selection, connection state, and play/stop controls.
- [ ] Keep the browser path receive-only and avoid direct `audio.ceruleanavi.net` credentials.
- [ ] Stream fixed 20 ms PCM frames over an authenticated HTTP response and schedule them through AudioContext.
- [ ] Verify the player with a local fake audio source.

### Task 5: Deployment and verification

**Files:**
- Create: listen service deployment manifests and environment example
- Modify: `can-voice/docs/deploy.md` and relevant CAN deployment docs

- [ ] Route HTTPS for `listen.ceruleanavi.net` to the web service.
- [ ] Keep direct UDP 64738 routing for `audio.ceruleanavi.net`.
- [ ] Configure service-to-service credentials through secret storage only.
- [ ] Run focused Go, Rust, and web tests plus a production-shaped health check.
