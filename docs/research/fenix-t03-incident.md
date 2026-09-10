# fēnix `t03` mounted-MTP incident

The ignored `.tmp/fenix-t03` capture records a backup-free update with 18 writes and 9 removals.

Latest inspection:

- all 18 write targets exist at their planned sizes;
- 3 removal targets are absent and 6 remain;
- no commit marker exists;
- 877,002,752 bytes were free.

No map needs another download. Resume this transaction, finish the six removals, and commit it.

The incident established the mounted-MTP acceptance rule: verify payload hashes on the host, wait for copy finalization,
then require the exact destination path, regular-file type, and size. Garmin validates map content after disconnect.
Recovery uses the same rule; only an ambiguous partial object requires a bounded prefix read before deletion.
