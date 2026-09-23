# Voice identity and radio authority

Date: 2026-09-23
Status: approved direction, implementation pending
Scope: can-api token issuer, can-voice server and clients, can-fsd observer presence

## Identity contract

Every voice session has an authenticated CAN ID. A normal pilot or controller
has one voice session for that ID. An ATIS fleet account may have one session
per configured ATIS callsign. The voice server accepts the extra ATIS session
key only when the signed ticket names that station.

The short-lived ticket carries a role (`pilot`, `observer`, `controller`, or
`atis`), a controller callsign or ATIS-only station callsign, and a bounded list of permitted TX
frequencies. The issuer obtains controller assignments from the live FSD feed.
It issues an ATIS station scope only to the configured fleet CAN ID for a
callsign in the configured ATIS station list. Requests for an unowned station
or frequency fail. Ordinary tokens retain empty station scope. The existing
`{token, expires_in}` response shape stays unchanged. Pilot and observer
tickets allow one client-selected in-band COM frequency at a time only while the same CID has
the matching FSD role online. Controller tickets name the online callsign and
its assigned frequency. ATIS tickets name one configured station and its
configured frequency. Passwords and tickets are not logged.

The server compares every requested station with the signed claim at HELLO.
It admits TX only on a ticket-authorized frequency, and XC only between
admitted TX frequencies. RX remains subject to the existing count limit.
Rejections appear in SUBACK. The server also checks live FSD assignment when
transmitting and after feed updates; assignment loss removes TX and XC. If the
authority feed is unavailable or stale, new TX is denied and existing TX is
stopped. Position-feed degradation may continue to fail open for reception,
but it cannot grant TX.

The server removes TX when the signed grant expires. The client renews by
reconnecting with a new ticket before expiry and resends its latest SUB.
A controller handoff or logoff removes the grant as soon as the server observes
it; ticket expiry bounds any delay caused by feed delivery. An RX-only session
may remain connected after its TX grant expires.

## Pilot and observer sessions

XPC and MSFS construct FSD and voice configuration from one pilot identity.
One owner starts and stops the FSD session, voice session, and simulator pump.
Connection failure releases every resource already started. The simulator
remains the source of position and COM1/PTT state.

An observer signs in to FSD with their own CAN ID and observer callsign using
facility 0. The observer publishes their local simulator latitude and longitude
to FSD. Voice range uses that observer's own FSD position, with an observer
radius capped at 100 NM regardless of client-declared visual range. Observer
mode no longer sends `HELLO.follow`
or borrows another member's position. The UI uses an observer callsign field
instead of a captain/follow field. Manual observer frequency still overrides
local COM1. Observer capabilities stay explicit: no flight plan, IDENT, or
aircraft injection; text messaging is enabled only if the observer FSD role
supports it. A missing simulator position leaves the observer without a
geographic range grant until a valid position arrives.

The wire decoder may accept an empty legacy `follow` field during migration.
The server rejects nonempty `follow` values. The two pilot clients stop
emitting the field before the strict server rollout. ATIS uses its authorized
station callsign for position lookup, without the general `follow` facility.

## Controller client

`audio-for-can` stores desired RX/TX/XC and replays it after reconnect. The
server's SUBACK remains the displayed effective state. Its feed polling can
suggest the staffed frequency and prevent accidental TX locally; it is not
the permission boundary. The API signs the active controller callsign and
frequency into the ticket. Token renewal uses the same scope and refreshes
the effective radio state when an assignment changes.

PTT, volume, audio devices, and reconnect stay in the shared bridge. A
controller can listen to additional frequencies within the RX limit. TX and
XC are limited to the server-approved assignment. Cross-coupling never makes
an unauthorized frequency a TX endpoint.

## Rollout and checks

Deploy the token issuer and client request support first. Deploy strict
server enforcement after ATIS and controller clients can request scoped
tickets. Old unscoped clients may receive RX but receive no TX grant. No
legacy nonempty station or follow exception remains after rollout.

Tests cover token claim equality, unauthorized seat requests, handoff and
feed loss, TX/XC rejection, renewal, ATIS fleet multiplicity, observer FSD
presence and own-position routing, and coordinated client teardown.

The reference implementations are xPilot commit `4bd5b80` and TrackAudio
commit `0a762875`. xPilot's observer signs in to FSD and uses local simulator
position. TrackAudio keeps desired radio state in its UI and delegates
effective state to the voice client. Neither client-side check is used as a
server authorization rule here.
