# Using Garmin Toolkit

## Install and open

Choose an experimental release whose signed inventory lists your architecture. Its acceptance reports record the tested
Home Assistant OS, Core, and Supervisor versions and checks for installation, ingress, Course downloads, restarts, and
backup/restore.

1. Add the repository to your Home Assistant instance. Confirm the repository URL in its app store.

   [![Add the Garmin Toolkit repository to Home Assistant][hass-install-badge]][hass-install]

2. Open Garmin Toolkit's app page, then install and start it.

   [![Open Garmin Toolkit's app page in Home Assistant][hass-app-badge]][hass-app]

3. Open its web interface through Home Assistant ingress.

My Home Assistant asks for your instance address. To add the repository manually, use
`https://github.com/kubijo/garmin-toolkit#hass-apps`.

## Verify downloads

Authenticate `SHA256SUMS` using the
[provenance instructions](https://github.com/kubijo/garmin-toolkit/blob/main/infra/release/README.md#signed-provenance),
then run `sha256sum --check SHA256SUMS` alongside the payloads. Releases also include source bundles and acceptance
reports.

## Make your first Course

1. Create a profile and select it.
2. Import a GPX file as a route. Open that route and generate its FIT Course.
3. Download the original GPX and generated FIT file for use outside Home Assistant.

USB transfer requires a tested device/firmware/host combination and VM passthrough when applicable. Installation and
route generation do not qualify USB access. For absent or unsupported devices, use the downloaded FIT file with a
supported transfer method.

## Data, updates, and recovery

Profiles, routes, imports, Courses, and device associations persist in private `/data`. The app assumes trusted local
operation; stored data and portable snapshots are plaintext.

Before updating, download a Home Assistant backup containing Garmin Toolkit. Supervisor stops the app during this cold
backup, then restarts it. Choose an update supported on your architecture.

Restore with the compatible app version specified by the release. Installing an older binary over newer data is
unsupported. Verify profiles, routes, and Course downloads afterward.

On Supervisor 2026.10.2, run `ha supervisor restart` after restoring a reinstalled app, then reopen its web interface.
This refreshes stale Supervisor ingress routing that can otherwise return HTTP 503.

Uninstalling removes app data; reinstallation starts empty until restored. External GPX/FIT downloads and backups remain
separate. Check the release's lifecycle acceptance results before upgrading or recovering.

## Troubleshooting and reports

- Missing app: check repository URL/branch, refresh the store, and confirm architecture support.
- Failed installation: check Supervisor logs and anonymous registry access.
- Failed web interface: confirm the app is running and open it through ingress.
- Missing device: check hardware support, cable, device mode, host attachment, and VM passthrough. Do not reset or
  format it.

Report [issues](https://github.com/kubijo/garmin-toolkit/issues) with app/OS/Core/Supervisor versions, architecture,
operation, and a short redacted log. Exclude tokens, names, device identifiers, locations, private activities, app data,
and backups.

[hass-app]: https://my.home-assistant.io/redirect/supervisor_app/?app=af63a031_garmin_toolkit
[hass-app-badge]: https://my.home-assistant.io/badges/supervisor_app.svg
[hass-install]: https://my.home-assistant.io/redirect/supervisor_add_addon_repository/?repository_url=https%3A%2F%2Fgithub.com%2Fkubijo%2Fgarmin-toolkit%23hass-apps
[hass-install-badge]: https://my.home-assistant.io/badges/supervisor_add_addon_repository.svg
