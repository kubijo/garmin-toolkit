# Domain map

## Identity and sources

| Term                | Meaning                                                                           |
| ------------------- | --------------------------------------------------------------------------------- |
| Person              | A human described by health, activity, or trip data.                              |
| User                | A Garmin Toolkit principal allowed to act on data.                                |
| Source              | One connector instance, such as an attached device or Garmin Connect account.     |
| Source identity     | An opaque identifier meaningful only within one source.                           |
| Device              | A physical unit, independent of how any source identifies it.                     |
| Device observation  | Source-provided model, firmware, identity, and capabilities acquired at one time. |
| Capability evidence | A result for one device variant, firmware, host, adapter, and operation tuple.    |

A user may represent a person but is not the person's record. Device observations are source assertions.

External identifiers never become Garmin Toolkit primary identities; ADR 0015 defines portable application identities.

## Recorded and planned data

| Term                | Meaning                                                                            |
| ------------------- | ---------------------------------------------------------------------------------- |
| Activity            | One recorded exercise or comparable completed event.                               |
| Trip                | User-curated activities, route plans, and notes; never inferred from FIT.          |
| Session             | A sport or phase within an activity.                                               |
| Lap                 | A source-declared subdivision of a session.                                        |
| Track               | Ordered spatial series, optionally timed for recorded activities.                  |
| Record              | One timestamped sample within a series.                                            |
| Route plan          | User-owned cycling or running plan, independent of source and device encoding.     |
| Route-plan revision | Immutable geometry or control points, deployable metadata, cues, and provenance.   |
| FIT Course          | Device-specific encoding of a deployable route-plan revision.                      |
| Device deployment   | One revision's generated artifact and transfer/acceptance evidence for one device. |
| Workout             | Planned structured exercise steps.                                                 |
| Measurement         | A typed point or interval observation.                                             |

Measurements include weight, heart rate, HRV, sleep, and skin temperature.

Preserve source structure. Normalization represents unknowns but invents no sessions, laps, people, or capabilities.

## Evidence and derivation

An **artifact** is immutable source or generated material with its hash and media type. An **observation** is a timed
source assertion that may cite an artifact; provenance records its source, identity, acquisition, parser, and
transformations. Route-plan revisions cite their inputs without becoming observations.

An **association** links observations that may describe one subject without merging them. A **conflict** is their
disagreement. A **derived view** is reproducible output retaining its inputs.

All connectors produce peer observations; none has source-wide precedence.

[ADR 0019](../decisions/0019-relational-reconciliation.md) defines relational provenance, semantic fingerprints, and
non-merging association groups. [ADR 0028](../decisions/0028-user-owned-route-plans.md) defines route plans and device
deployments. The Garmin Connect stage owns sharing.
