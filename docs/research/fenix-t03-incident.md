# Resolved fēnix `t03` mounted-MTP incident

The ignored `.tmp/fenix-t03` capture records a backup-free update with 18 writes and 9 removals.

The original interruption left:

- all 18 write targets exist at their planned sizes;
- 3 removal targets are absent and 6 remain;
- no commit marker exists;
- 877,002,752 bytes were free.

Recovery reused the 16.92 GB retained payload set, accepted all 18 write targets by path and size, removed the six
remaining obsolete targets, and committed the transaction. Verification took 43 seconds; reconciliation took 30 seconds.
Free space rose to 11.07 GB. No map payload was downloaded or read back from the watch.

The incident established the mounted-MTP acceptance rule: verify payload hashes on the host, wait for copy finalization,
then require the exact destination path, regular-file type, and size. Garmin validates map content after disconnect.
Recovery uses the same rule; only an ambiguous partial object requires a bounded prefix read before deletion. A safe
restart on 2026-09-11 remounted firmware 22.44 without a recovery prompt; the normal component inventory retained the
recovered 2026.11 maps.
