# Using Garmin Toolkit

## Install and open

1. Use a published experimental release from the [project releases](https://github.com/kubijo/garmin-toolkit/releases).
   Check its supported architecture and Home Assistant versions before installing. Release candidates are for testing
   and are not a published installation path.
2. Add the repository URL given in that release to Home Assistant's app store. The planned public URL is
   `https://github.com/kubijo/garmin-toolkit#hass-apps`; it is not ready until a release explicitly advertises it.
3. Find **Garmin Toolkit**, install it, and start it. Open its web interface through Home Assistant. Home Assistant
   handles access to ingress; the app does not expose a separate host port.

## Make your first Course

1. Create a profile and select it.
2. Import a GPX file as a route. Open that route and generate its FIT Course.
3. Download the original GPX and generated FIT file. Keep these files independently of the app backup if you need them
   outside Home Assistant.

Device attachment is optional for this workflow. For USB transfer, connect the Garmin device to the Home Assistant host
and use only a device/firmware/host combination listed as tested in the release. An app running in a VM needs explicit
device passthrough. Network installation and route generation do not verify physical USB access. If a device is absent
or unsupported, download the FIT file and use a separately supported transfer method.

## Data, updates, and recovery

Persistent app data lives in the app's private `/data` directory. Profiles, routes, original imports, generated Courses,
and device association records belong to that data. The app is intended for trusted local operation. Stored data and
portable snapshots are plaintext; retain exported files and backups accordingly.

Create and download a Home Assistant backup that includes Garmin Toolkit before updating. The app uses cold backup:
Supervisor stops it while copying data, then starts it again. Update through Home Assistant only to a release supported
on your architecture.

For recovery, restore the backup with the compatible app version recorded by the release instructions. Installing an
older executable over newer app data is not a supported downgrade. Verify your profile, route, and Course downloads
after restoration.

On Supervisor 2026.10.2, run `ha supervisor restart` after restoring a reinstalled app, then reopen its web interface.
This refreshes stale Supervisor ingress routing that can otherwise return HTTP 503.

Uninstalling removes the installed app and its app data. Reinstallation starts empty unless you restore a backup.
Downloaded GPX/FIT files and backups stored outside the app remain separate. Check the release's lifecycle acceptance
results before relying on upgrade or recovery behavior.

## Troubleshooting and reports

- If the app is missing from the store, check the exact repository URL and branch suffix, refresh the store, and check
  whether the release supports your architecture.
- If installation fails, check Supervisor logs and anonymous access to the image registry. A public release should not
  require the maintainer's registry credentials.
- If the web interface fails, confirm the app is running and open it through Home Assistant ingress.
- If a Garmin device is missing, check the release's hardware support matrix, cable, device mode, host attachment, and
  VM passthrough. Do not reset or format the device as a troubleshooting step.

Report failures through [GitHub issues](https://github.com/kubijo/garmin-toolkit/issues). Include the app version,
architecture, Home Assistant OS/Core/Supervisor versions, operation attempted, and a short redacted log excerpt. Remove
tokens, names, device identifiers, locations, and private activity files. Do not attach an app data directory or backup.
