# Desktop packaging evidence

The recorded
[host checks](https://github.com/kubijo/garmin-toolkit/blob/28a1bb2bc296b69e8cb8a5471292c0e3a1488234/docs/architecture/build-system.md#desktop-packaging)
used the x86_64 demo AppImage on Ubuntu 26.04 with an NVIDIA RTX 4090 driving the display, driver 595.91.07, and Mesa
26.0.3. Packaging contracts and build commands live in
[the build system](../architecture/build-system.md#desktop-packaging); interaction methods live in
[developer tools](../architecture/developer-tools.md#scenario-runs-and-profiling).

## Verified scope

- NVIDIA/Vulkan passed `activity-smoke`, covering map interaction, playback, laps, scrubbing, and activity
  replacement/restoration. Process mappings showed bundled libc/Vulkan with the host NVIDIA driver.
- llvmpipe Vulkan passed with two worker threads and functional/background timing. Normal timing hit an interaction
  deadline, so this establishes functionality only.
- Backup saving, file-dialog cancellation, and opening the host log folder passed. Manual covered-window Developer tools
  interaction passed; regressions cover child input and lifecycle while the root is occluded.
- Debug-desktop and AppImage checks passed covered navigation without polling, cancellation, capture, and Import GPX
  through the status overlay. At `720×480`, `stationary-arrival` failed because `map.fit` was clipped.
- The local [headless probe](../development.md#headless-hardware-rendering-probe) passed activity and responsive-layout
  scenarios on AMD RADV/Mesa with 4x MSAA. Inspected wide/narrow captures contained maps, routes, charts, text, and
  controls. This excludes native presentation and the AppImage library mix.

## Cross-GPU presentation failure

Forcing the AMD integrated GPU with no AMD-connected display failed DMA-buffer import on Wayland and surface creation on
XWayland. The Wayland failure reproduced outside the AppImage and with Homebrew's `vkcube --wsi wayland` forced to the
host Radeon ICD, independently of this application and its bundled libraries. The host's AMD-to-NVIDIA path is therefore
a separate reproduction; AMD-driven displays and other hosts remain unqualified.

This evidence covers one x86_64 demo AppImage host. Other hosts, aarch64, production USB/GVfs, and Flatpak permissions
require the acceptance in the [publication plan](../plans/device-expansion-and-publication.md).
