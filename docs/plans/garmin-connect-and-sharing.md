# Garmin Connect and sharing

Add optional Garmin Connect and multi-user sharing without weakening local operation, provenance, or USB.

## Work

1. Revalidate ADR 0008, authentication, terms, and limits before implementation.
2. Build a replaceable opt-in adapter with conservative polling and actionable failures; outages cannot disable USB.
3. Associate USB and Connect observations while retaining both sources, originals, and provenance.
4. Resolve OQ-019. Sharing must be explicit, authorized, revocable, exportable, and snapshotted.
5. Map external identities without using them as application primary keys.
6. Threat-model credentials, sessions, user separation, restored snapshots, and shared data.

[Security hardening](security-hardening.md) must first provide target credential custody. Configuration stores only
opaque references; application services own authorization. Sharing requires negative isolation and revocation tests.

Delete this plan after Connect failures cannot harm local USB use, secrets cannot reach captures or exports, sharing
isolation is tested, and every support claim names its evidence and unofficial boundary.
