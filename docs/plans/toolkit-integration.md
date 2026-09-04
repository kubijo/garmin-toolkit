# Cross-cutting integration gates

Apply these gates while closing each working slice. Finish
[single-path execution](../decisions/0037-single-execution-path.md) and preserve
[existing state and identities](../decisions/0036-persistent-identity-compatibility.md).

## Remaining work

1. Verify packaged production/demo IDs, data roots, and binary names. Exercise desktop fallback with a populated legacy
   database and HASS environment precedence through the built applications.
2. Measure device/update/CLI coverage after the [mounted-update tests](mounted-device-updates.md). Raise the enforced
   floor through behavioral coverage, without blanket exclusions or suppression.
3. Audit crate boundaries after the transaction work. Keep a crate only for an independently reusable capability or a
   dependency-inversion boundary. In particular, keep `garmin-progress` limited to operation observation and
   cancellation; move policy, persistence, and presentation to their owners.
4. Measure translation adoption separately from catalog completeness. Inventory production UI surfaces, route their
   user-visible prose through typed FormatJS descriptors, and reject unregistered copy mechanically. Review English
   source prose before translating it; do not preserve poor wording merely to keep a catalog ID stable.

## Exit criteria

- Existing state and persisted plans remain usable under an evidenced compatibility contract.
- Built identities, sandboxed checks, security audit, and behavioral coverage have revision-specific evidence.

Hardware proof: [mounted-device updates](mounted-device-updates.md). Capacity discovery:
[device state](device-state.md). Delete this plan once its remaining contracts and evidence have durable homes.
