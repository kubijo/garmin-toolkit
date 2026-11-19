# Desktop file association fixtures

- `2026-09-07-coastal-run.fit` shows a GPS route, elevation, and heart rate.
- `indoor-power-ride.garmin-activity` shows power and cadence without a GPS route.

Both come from the [development activity corpus](../fit/development-activities/fixture.toml).

Open Garmin Toolkit once so Finder discovers it. Use **Open With → Garmin Toolkit** for each fixture:

1. After quitting with Command-Q. The app should launch and show the preview.
2. While the app is running. The preview should open in the existing window.
3. While `result/bin/garmin-desktop` is running from a terminal. The preview should open in that instance.

Dismiss each preview between attempts. On a fresh installation, verify that the preview opens before any profile
exists. Click **Choose a profile**, create one, and confirm the import once the preview shows the destination.

Regenerate the fixtures from the repository root with a fresh output directory:

```sh
cargo run --locked -p garmin-services --example file_association_fixtures -- /tmp/garmin-file-associations
```

Generated provenance IDs can change the package bytes between runs.
