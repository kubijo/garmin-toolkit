# Active plans

Plans contain unfinished work only. Completion requires evidence, durable documentation, and removal of the plan.

All plans obey [single-path execution](../decisions/0037-single-execution-path.md) and
[validated interface previews](../decisions/0038-validated-interface-previews.md).

## Execution stages

Complete each stage as a working vertical slice before advancing.

| Stage | Slice                                                                   | Working result                                                          |
| ----- | ----------------------------------------------------------------------- | ----------------------------------------------------------------------- |
| 01    | [Read-only device inspection](device-state.md)                          | Recognized devices are inspected and presented automatically.           |
| 02    | [Portable data foundation](data-foundation.md)                          | Verified snapshots, restore, open export, and GPX sharing work.         |
| 03    | [Home Assistant watch slice](hass-watch-vertical-slice.md)              | The deployed add-on imports FIT, plans a route, and transfers a course. |
| 04    | [Shared interface workflows](shared-interface-workflows.md)             | Desktop and HASS complete the same reviewed workflows.                  |
| 05    | [Security hardening](security-hardening.md)                             | Optional protection works without removing passwordless local use.      |
| 06    | [Garmin Connect and sharing](garmin-connect-and-sharing.md)             | Opt-in cloud and sharing cannot weaken local USB operation.             |
| 07    | [Device expansion and publication](device-expansion-and-publication.md) | Every published device and application claim has current evidence.      |
| 08    | [CLI distribution channels](distribution.md)                            | Verified archives and accepted installation channels are published.     |

[Cross-cutting integration gates](toolkit-integration.md) apply at every stage exit rather than forming an incomplete
user-facing stage. Each stage must use the production path through injected adapters, pass boundary tests and relevant
interface previews, move lasting evidence into durable documentation, and remove completed plan material. Run
`just qa::full` and `just qa::audit` before closing it; record pushed-revision package evidence where the stage claims a
deployable artifact.

Read-only device inspection owns capacity discovery and display. Completed transaction preflight and hardware-write
evidence lives in [USB synchronization](../research/usb-sync.md#map-maintenance-evidence). Device expansion owns support
claims and HASS/desktop publication; distribution owns CLI packaging.

The consented fēnix and Edge hardware update, restart, and interrupted-recovery matrix is complete. HASS and desktop map
mutation remain owned by their active interface and publication plans; their existing device inspection and capacity
views are read-only.

[Open questions](open-questions.md) records unresolved choices under their earliest owning plan.
