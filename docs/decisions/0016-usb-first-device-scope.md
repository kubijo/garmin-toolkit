# 0016: USB-first device scope

## Decision

Limit initial device support to watches and bike computers with USB data transfer. Prioritize activity and aggregate
review plus computer-based course planning and consented upload. Defer the Index S2 and other devices without USB data
access.

This supersedes ADR 0007's scale ordering; its watch and bike-computer order remains.

## Why

Watches and bike computers record data away from the host, then expose a shared local USB/FIT path. They support the
primary offline workflow without cloud or wireless provisioning.

## Consequences

The initial matrix covers fēnix 8, Venu 3S, Edge 850, and Edge 1050. Scale, BLE-only, and cloud-only adapters require a
later decision and do not shape the first storage or service contracts.
