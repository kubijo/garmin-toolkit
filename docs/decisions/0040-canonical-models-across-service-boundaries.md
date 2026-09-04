# 0040: Canonical models across service boundaries

## Decision

`garmin-model` owns the domain vocabulary shared by native and WASM code. Adapters map external data into these models;
workflows, persistence, RPC contracts, and UIs pass the owned models onward. `garmin-service-api` may define RPC
envelopes, state, and errors, but must not copy domain types for transport.

Serialized model values must round-trip through non-self-describing Postcard. Model enums therefore use Serde's
externally tagged representation. A different representation requires replacing the wire codec or proving equivalent
native and WASM behavior before this decision is superseded.

This narrows ADR 0024's “owned messages” to RPC envelopes, state, and errors. Domain values remain owned by
`garmin-model`.

## Enforcement

The source-shape gate rejects internally tagged, adjacently tagged, and untagged enums plus field flattening and
serialization-only omission in `garmin-model`. It also rejects new public structs, enums, and aliases in
`garmin-service-api` until the RPC-envelope allowlist is deliberately reviewed. Postcard round-trip and live host/client
tests cover the values crossing the current service boundary.

## Why

Transport mirrors split one concept into models that can drift. A canonical, portable model keeps platform and protocol
conversion at adapters while preserving one type through every application.
