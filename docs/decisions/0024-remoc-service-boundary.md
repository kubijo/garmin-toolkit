# 0024: Remoc service boundary

## Decision

Use [Remoc](https://github.com/remoc-rs/remoc) for the typed service boundary: local connections for native clients and
binary WebSockets through HASS ingress.

Keep owned messages in `service-api` and Remoc out of lower crates. Persist no wire representation. Pin the workspace
version, disable defaults, and enable only used features.

Prove real ingress, cancellation, reconnect, slow consumers, bounded transfer, version mismatch, and hostile frames.
Require Miri only for owned unsafe code and Loom only for owned concurrency primitives.

## Why

Remoc combines Rust traits, transferable typed channels, local operation, WASM, and framed transports. Its young browser
stack requires containment and adversarial tests. `jsonrpsee` is the fallback; tarpc needs browser/streaming glue and
gRPC-Web lacks the required bidirectional WebSocket path.

## Consequences

The wire protocol is private and ephemeral. Replacing Remoc may rewrite `service-api` and adapters, never data or lower
layers.
