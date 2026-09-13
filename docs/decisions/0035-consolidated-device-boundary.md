# 0035: Consolidated device boundary

## Decision

Use `garmin-device` as the only shared device boundary. It owns attachment discovery, hotplug reconciliation, exact
manifest retention, typed capabilities, mass-storage access, raw MTP access, mounted-MTP access, safe paths, capacity
inspection, and bounded file operations.

Attaching or mounting a recognizable Garmin authorizes automatic local inspection of canonical `GarminDevice.xml` and
storage metadata. Inputs remain untrusted; inspection does not crawl or copy unrelated content.

Copying user files from the device, contacting a network service, and every device mutation remain separate explicit
actions. Frontends may provide target adapters such as Linux GIO monitoring, but do not define a second device model or
an additional "read device details" consent step.

The strict typed `GarminDevice.xml` view and the exact source document coexist: applications use allowlisted
capabilities, while the map service receives the original manifest it requires.

Device-metadata parsing has one format-neutral boundary. It detects a document from its root element and namespace,
dispatches to a statically registered format parser, and returns normalized metadata tagged with its source format and
any tolerated quirks. GarminDevice v2 is currently the only registered format; its private wire structures and
normalization policy live under `manifest::garmin::v2`. Future v1, v3, or other-manufacturer support adds a separately
named parser and dispatch arm rather than model-name conditionals or changes to the v2 parser. Metadata-file discovery
remains Garmin-specific until a real additional format establishes its own locations.

This supersedes ADR 0012 where its former USB crate boundary conflicts with the consolidated device owner.

## Why

The merged sources contained complementary read-only discovery and hardware-tested update stacks. Keeping both would
duplicate identity, manifest, MTP, and lifecycle behavior at the most safety-sensitive boundary.

## Consequences

All device transports fail closed on ambiguous manifests, unsafe paths, partial listings, identity changes, and
disconnects. Updates, backups, removal, recovery, activity import, and future device-state views consume the same device
types. Parser quirks describe tolerated wire-layout deviations; they do not create model-specific parsers or alter the
normalized capability contract. Physical-device validation remains necessary for transport claims that simulators cannot
prove.
