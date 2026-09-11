# Cross-cutting integration gates

Apply these gates while closing each working slice. Finish
[single-path execution](../decisions/0037-single-execution-path.md). Before the first release, change development-only
state formats directly rather than adding compatibility branches.

## Remaining work

1. Verify packaged production/demo IDs, data roots, and binary names. Exercise desktop startup with a populated database
   and HASS environment precedence through the built applications.
2. Measure device/update/CLI coverage after the completed
   [mounted-update work](../research/usb-sync.md#map-maintenance-evidence). Raise the enforced floor through behavioral
   coverage, without blanket exclusions or suppression.
3. Audit crate boundaries after the transaction work. Keep a crate only for an independently reusable capability or a
   dependency-inversion boundary. In particular, keep `garmin-progress` limited to operation observation and
   cancellation; move policy, persistence, and presentation to their owners.
4. Measure translation adoption separately from catalog completeness. Inventory production UI surfaces, route their
   user-visible prose through typed FormatJS descriptors, and reject unregistered copy mechanically. Review English
   source prose before translating it; do not preserve poor wording merely to keep a catalog ID stable.

## Exit criteria

- Current state and persisted plans use one versioned format without pre-release compatibility branches.
- Built identities, sandboxed checks, security audit, and behavioral coverage have revision-specific evidence.

Hardware proof: [USB synchronization](../research/usb-sync.md#map-maintenance-evidence). Capacity discovery:
[device state](device-state.md). Delete this plan once its remaining contracts and evidence have durable homes.
