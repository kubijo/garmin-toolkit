# Backup and restore

Select the owner profile, then **Backup and restore** from the profile menu.

A backup contains all profiles, activities, original imported files, route revisions, and profile pictures. Restoring
replaces the whole database; it does not merge into the selected profile. Save a current backup first if you need to
keep the existing data. Backups are unencrypted; device pairing and cloud credentials are excluded. An existing
on-device pairing marker is not changed by backup or restore. Review its profile association after restoring different
users.

## Desktop

- **Save backup** writes a `.tar.zst` snapshot to the location chosen in the save dialog. Confirm replacement in the
  dialog when selecting an existing file. Save outside the application's data directory.
- **Open backup** reads and verifies a snapshot. Check its creation date, application version, and database size, then
  choose **Replace all data**. Opening a file alone does not change your data.
- Cancel before confirming restore to keep the current database. Once replacement starts, keep the application open.
  After restoration, choose a profile from the restored data.

## Home Assistant

**Create a backup** offers **Download to this device** and **Save on server**. **Restore a backup** offers **Upload from
this device** and **Open from server**. Download and upload use the browser's download manager and file picker. By
default, server actions open the application's file chooser in a separate browser window over directories visible to the
HASS process. Set **File browser windows** to **Inside the app** in profile settings to keep the chooser in the app
canvas instead. If popups are blocked, retry or open the chooser in a tab. Closing the chooser makes no selection;
selecting a backup returns to the parent tab for review and explicit restore approval. File contents stay on the server.
Saving over an existing server file requires confirmation. Save destinations inside the application data root are
rejected.

Browser downloads report when delivery starts; check the browser's download manager for completion. Reconnecting resumes
an active operation and invalidates earlier restore approval. Restarting the host discards unfinished transfers.

After reloading the browser, select the owner profile and open **Backup and restore**. **Previous backup operations**
lists retained work. Resume a verified backup to review it and confirm restoration again, or discard it to free its
temporary files. An incomplete browser upload must be discarded and its file selected again. Completed operations can be
dismissed. Work in use by another window or download cannot be reclaimed; refresh after it closes. An approved restore
can be monitored but cannot be discarded during database switching.

## Recovery and compatibility

Restore requires a compatible database schema; incompatible or damaged snapshots are rejected before replacement. A
failed or interrupted switch recovers the previous database, including an approved switch after a host restart. If
recovery cannot open it, the application reports an error rather than creating an empty database.

Desktop and HASS demo modes use separate data directories and preserve changes and restores across restarts.
