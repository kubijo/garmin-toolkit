# 0027: Deferred HASS data publication

## Decision

Do not publish Nimrag data as HASS entities, events, or external statistics in the initial milestones. HASS hosts and
opens Nimrag but receives no domain data.

Preserve the option through stable application-owned identifiers, target-neutral domain events and read services, and a
HASS adapter boundary. Put no HASS entity, recorder, or MQTT semantics in core models.

## Why

There is no current automation or dashboard use. Choosing a public HASS contract now would create unsupported API and
retention commitments.

## Consequences

The first HASS slice exposes only Nimrag's own UI and operational health required to run it. Data publication needs a
future use case and a superseding decision; it is not an open question or current execution task.
