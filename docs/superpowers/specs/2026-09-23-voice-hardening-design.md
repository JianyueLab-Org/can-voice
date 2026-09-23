# Voice transport and runtime hardening

Date: 2026-09-23
Status: approved direction, implementation pending
Scope: remaining findings from the 2026-09-23 can-voice review

## Protocol and session limits

The QUIC listener bounds concurrent connections waiting for authentication.
It rejects excess connections before starting per-connection workers. The
limit is configurable with a conservative default and exposes an aggregate
rejection counter without retaining peer identifiers. The handshake deadline
still applies inside the limit.

The FSD reader rejects an individual line above a fixed byte limit before
allocating its entire contents. Oversized input closes that FSD session with
an error. Ordinary packet framing and reconnect behavior remain unchanged.

Every sent SUB has one matching SUBACK. The client serializes SUB messages
until the preceding ACK arrives, while retaining the latest desired
declaration. UI denial lists compare an ACK with the declaration it confirms.
Disconnect clears pending and acknowledged state. A delayed ACK from a prior
connection cannot update a new connection.

The router records a talker announcement only for a listener still present
in its session table. Removing a listener cannot leave an `announced` entry
behind. Timed-out METAR callers are removed from the waiter map even if FSD
stays connected. Outstanding requests have a fixed cap. The pilot client's
unused callsign `seen` map is removed.

## Simulator and playback safety

X-Plane RREF receives data only from the selected simulator socket address.
Packets from another source cannot update aircraft or radio state.

Every SimConnect dispatch is checked against its reported byte length before
unsafe casts or variable-length reads. A truncated packet is ignored with a
bounded diagnostic; it never dereferences unavailable bytes.

The jitter buffer rejects stale packets outside its reorder window before
updating tail and sequence state. A tail from an earlier talkspurt cannot end
the current one. Normal late, duplicated, and wrapped sequence behavior is
covered by regression tests.

The audio device callback does not allocate temporary vectors on the real
time thread for F32 or I16 output. Scratch storage is prepared before the
callback or conversion writes directly into the supplied buffer. Playback
content and channel layout are unchanged.

## ATIS and deployment

ATIS text and decoded PCM have explicit size and duration limits before TTS
or playback. Child processes have timeouts and are terminated on task drop.
Temporary media are removed on success, error, and cancellation. Invalid or
oversized feed entries do not stop other stations.

The deployment guide names the actual `CAN_VOICE_FSD_FEED` Compose variable.

Focused regression tests precede each code change. Final checks include the
Rust workspace tests and clippy, Go race tests and vet, all four frontend
builds, and release script tests. Platform-only SimConnect and audio behavior
also receives source-level length and callback tests where CI lacks a device.
