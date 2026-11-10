# Garmin GFDI wire protocol

Transport-independent framing, GFDI envelopes, route selection, bounded MultiLink registration, reliable MLR transport,
device-identity observation, and typed FileAccess messages. There is no Bluetooth, OS, async runtime, application
database, or other workspace crate dependency. No component discovers or connects to a device. Message codecs are not a
command allowlist; transfer policy and device-state permissions belong above the wire layer.

`FrameDecoder::push` accepts bytes across arbitrary notification boundaries and emits each completed frame or error
immediately. The caller supplies an encoded-body size limit and must reset partial input on disconnect or timeout. COBS
only detects structural errors; `Message::decode` additionally checks total length and CRC before dispatch. Neither
provides authentication or replay protection.

`select_route` inspects caller-supplied characteristics and requires complete receive/send pairs with suitable
properties in the same service and slot. `Registration::begin` returns one explicit GFDI registration request; `receive`
correlates replies by client and service, while `poll` enforces a caller-supplied monotonic deadline. Cancellation is
local. Mode changes are rejected with the allocated channel retained in the error for explicit cleanup. No close-all,
retry, authentication, or automatic fallback is generated. The caller still needs authorization before sending the
request.

`MlrReceiver` delivers only in-order fragments and emits transport ACKs after five packets or twenty milliseconds;
duplicates and gaps trigger the current expected sequence. `IdentityProbe` composes registration, reassembly, CRC
validation, and device-information decoding. It sends no GFDI application replies and stops on authentication requests.
Its only cleanup packet closes the known handle allocated to its own client ID. This is identity observation, not a
complete Garmin companion handshake or activity sync.

`MlrSender` bounds queued and unacknowledged payload bytes, validates cumulative ACKs, and fragments into an initial
32-packet window. Retransmission halves the window and backs off from one to twenty seconds; an explicit retry budget
stops stalled transfers. Duplicate/stale ACKs cannot postpone retries. The caller must enforce a total deadline, send
returned packets in order, and cancel on transport failure. The identity probe does not use this application sender.

`proto` exposes Buffa-generated proto2 FileAccess messages from the [pinned schemas](proto/README.md). Buf owns schema
linting and formatting. FileAccess is the download target; no legacy file-index fallback is planned. The generated
schemas include mutation messages, so only audited operations belong in the download client.

`rpc` validates GFDI Protobuf fragment metadata, binds request identity and total size, and reassembles under explicit
size/deadline limits. Exact fragment replays receive another acknowledgement; conflicting or reordered data stops the
assembly. Transport ACKs, GFDI sequences, and Protobuf request IDs remain distinct.

`DownloadSession` composes those pieces into an unfiltered paginated listing and one activity pull through service
`0x2018`. It validates result presence, transport/compression negotiation, pipe start, bounded decompression, and exact
advertised size. Its host-information response names Garmin Toolkit; it never sends time/settings, authentication,
legacy reads, archive/delete, or synced-flag changes. The adapter validates FIT and publishes the original bytes only
after owned-handle cleanup and disconnect succeed. The command remains physically unqualified.

The optional [Linux smoke harness](../garmin-device/examples/gfdi-smoke.rs) uses this engine with a real adapter or a
synthetic peer. Commands and physical restrictions are in the
[adapter research](../../docs/research/bluetooth-adapters.md#identity-smoke-command).

`PairingSession` is a separate first-companion setup flow: host information, minimal FIT-link/MultiLink capabilities,
auth-negotiation acknowledgement and the three setup-completion events. It requires correlated sequenced command ACKs,
stops on encryption requests, and sends no settings, time or file operations. The Linux `pair` command owns OS bonding
through the desktop agent. Neither offline tests nor a successful OS bond qualify physical Garmin setup.

## Source and attribution

Protocol behavior and conformance case selection are adapted from
[Gadgetbridge revision 28cea5443cfa35ff1dfe047645e7abf18ced24cc][source]:

- `service/devices/garmin/communicator/CobsCoDec.java`: Garmin delimiters and terminal empty blocks.
- `service/devices/garmin/messages/GFDIMessage.java`, including its nested `MessageReader`: envelope and sequenced
  types.
- `service/devices/garmin/ChecksumCalculator.java`: CRC behavior, implemented here using the published `crc` crate.
- `service/devices/garmin/communicator/v2/MlrCommunicator.java`: sequence/handle bits, gap/duplicate ACKs, the 32-packet
  send window, retransmission/backoff, five-packet receive threshold, and twenty-millisecond ACK delay. Our receive
  timer starts at the oldest unacknowledged fragment. Unlike upstream, ACK progress is validated before expiring
  fragments, duplicate ACKs do not restart timers, and retries and buffered payload have explicit limits.
- `service/devices/garmin/messages/DeviceInformationMessage.java` and `GarminByteBufferReader.java`: device-initiated
  message 5024 and UTF-8 strings with one-byte lengths. The identity probe never replies; the download session adapts
  the host-information response with explicit toolkit names.
- `GarminSupport.completeInitialization()`, `messages/ConfigurationMessage.java`, `messages/SystemEventMessage.java`,
  `messages/status/AuthNegotiationStatusMessage.java`, and `devices/garmin/GarminCapability.java`: first-pairing setup,
  capability encoding and auth-negotiation response. Our handshake advertises only FIT link and MultiLink, omits
  settings/time sync, and requires correlated acknowledgements rather than assuming setup succeeded after writes.
- `service/devices/garmin/messages/ProtobufMessage.java`, `messages/status/ProtobufStatusMessage.java`, and
  `ProtocolBufferHandler.java`: Protobuf fragment metadata and short/extended acknowledgements. Reassembly adds explicit
  bounds, correlation checks, and exact-replay handling.
- `service/devices/garmin/FileSyncServiceHandler.kt` and `GarminSupport.downloadFileFromServiceV2()`: FileAccess
  listing, pull and transfer-status flow, the six-byte pipe request, three-byte start, and zlib data. Automatic
  synced-flag updates, time/configuration replies, and legacy flushes are excluded.
- `service/devices/garmin/communicator/v1/CommunicatorV1.java` and `communicator/v2/CommunicatorV2.java`: known routes,
  registration wire layout, client/service correlation, and mode flags. Synthetic tests cover all five MultiLink slots
  and both direct UUID families. They do not establish pairing compatibility or phone coexistence.
- `service/devices/garmin/communicator/CobsCoDecTest.java` under the upstream test tree: zero runs, long blocks,
  chunking, malformed frames, resynchronization, and overflow recovery. Tests use synthetic data instead of upstream
  device captures containing identity, location, or authorization fields. The short status-frame vector contains
  protocol fields only; it is retained to check the envelope independently of our encoder.

Main paths are below `app/src/main/java/nodomain/freeyourgadget/gadgetbridge/`; tests use `app/src/test/java/`. Upstream
attribution: Copyright (C) 2023–2024 Petr Kadlec; 2024–2026 beautiful_orca, Daniele Gobbetti, José Rebelo, Thomas
Kuehne. The adapted behavior and tests remain AGPL-3.0-or-later, matching this crate's license. COBS byte stuffing uses
the published MIT/Apache-2.0 `cobs` crate rather than a Java algorithm port.

This decoder handles multiple coalesced frames and reports malformed/oversized frames instead of copying upstream's
single-result buffering and silent reset behavior. It accepts valid minimal COBS frames independently of the GFDI
six-byte envelope minimum. No physical interoperability is claimed by the offline tests.

Remaining work is owned by the [download plan](../../docs/plans/garmin-bluetooth-downloads.md).

[source]: https://codeberg.org/Freeyourgadget/Gadgetbridge/commit/28cea5443cfa35ff1dfe047645e7abf18ced24cc
