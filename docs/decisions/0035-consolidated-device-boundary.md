# 0035: Consolidated device boundary

## Decision

Use `garmin-device` as the only shared device boundary. It owns attachment discovery, hotplug reconciliation, exact
manifest retention, typed capabilities, mass-storage access, raw MTP access, mounted-MTP access, safe paths, capacity
inspection, and bounded file operations.

Discovery may expose transport metadata without reading device contents. Inspection requires consent; mutation requires
a separate confirmed capability. Frontends may provide target adapters such as Linux GIO monitoring, but do not define a
second device model.

The strict typed `GarminDevice.xml` view and the exact source document coexist: applications use allowlisted
capabilities, while the map service receives the original manifest it requires.

This supersedes ADR 0012 where its former USB crate boundary conflicts with the consolidated device owner.

## Why

The merged sources contained complementary read-only discovery and hardware-tested update stacks. Keeping both would
duplicate identity, manifest, MTP, and lifecycle behavior at the most safety-sensitive boundary.

## Consequences

All device transports fail closed on ambiguous manifests, unsafe paths, partial listings, identity changes, and
disconnects. Updates, backups, removal, recovery, activity import, and future device-state views consume the same device
types. Physical-device validation remains necessary for transport claims that simulators cannot prove.
