# Production CLI map maintenance

The shared update engine, recovery policy, and progress UI are implemented. Automated evidence lives in
[device capacity and recovery](../research/device-capacity-and-recovery.md). Hardware evidence remains incomplete.

## Current recovery

The [fēnix `t03` transaction](../research/fenix-t03-incident.md) has all 18 writes at their planned sizes. Resume its
retained capture to finish six removals and commit; do not create a new plan or redownload a map.

The production TUI detects portable device state after device selection. It offers recovery or a proven clear before
contacting Garmin. A host receipt covers failure before portable state publication. Cross-host recovery still requires
payload reacquisition from recorded download identities.

## Hardware proof remaining

1. Complete the 256 MB disposable fēnix link probe: upload, read back, hash, remove, and clear its receipt.
2. Complete a small fēnix 8 Solar update, inspect the journal and destination metadata, disconnect safely, and confirm
   firmware acceptance.
3. Interrupt and recover a disposable fēnix transaction through the production TUI.
4. Repeat update, restart, acceptance, and recovery on Edge 1050.
5. Record hardware, firmware, adapter, revision, component, and outcome in [USB evidence](../research/usb-sync.md). Keep
   raw captures local.

Delete this plan only after both devices have black-box update, restart, and interrupted-recovery evidence.
