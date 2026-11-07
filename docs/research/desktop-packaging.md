# Desktop packaging evidence

Host: Ubuntu 26.04, x86_64, NVIDIA RTX 4090 driving the display, driver 595.91.07, Mesa 26.0.3. Build commands:
[build system](../architecture/build-system.md#desktop-packaging). Earlier results:
[host checks](https://github.com/kubijo/garmin-toolkit/blob/28a1bb2bc296b69e8cb8a5471292c0e3a1488234/docs/architecture/build-system.md#desktop-packaging).

## Verified scope

- NVIDIA/Vulkan passed `activity-smoke`, covering map interaction, playback, laps, scrubbing, and activity
  replacement/restoration. Process mappings showed bundled libc/Vulkan with the host NVIDIA driver.
- llvmpipe Vulkan passed with two worker threads and functional/background timing. Normal timing hit an interaction
  deadline, so this establishes functionality only.
- Backup saving, file-dialog cancellation, and opening the host log folder passed. Manual covered-window Developer tools
  interaction passed; regressions cover child input and lifecycle while the root is occluded.
- Debug-desktop and AppImage checks passed covered navigation without polling, cancellation, capture, and Import GPX
  through the status overlay.
- On 2026-11-04, the demo AppImage passed `stationary-arrival` at `720×480` and `activity-smoke` at `1100×720`, with
  NVIDIA/Vulkan and 4x MSAA. Captures were inspected. The
  [workspace regression](../../crates/garmin-ui/src/activity/workspace.rs) covers scrolling import controls into view at
  minimum size.
- The local [headless probe](../development.md#headless-hardware-rendering-probe) passed activity and responsive-layout
  scenarios on AMD RADV/Mesa with 4x MSAA. Inspected wide/narrow captures contained maps, routes, charts, text, and
  controls. This excludes native presentation and the AppImage library mix.

## Production AppImage: Edge 1050, 2026-11-05

- Direct USB/MTP discovery and inspection passed.
- The Activities bookmark opened `Internal Storage / Garmin / Activities` and listed FIT files with sizes.
- FIT activity preview loaded date, sport, distance, duration, and ascent, with an explicit Import FIT action.
- The rebuilt preview appeared inside the file window. Compact header and edge-to-edge content were accepted.
- Automated regressions cover Escape dismissal, location/selection preservation, and blocked browser shortcuts beneath
  the preview.

## ARM64 non-Nix Ubuntu VM: Edge 1050, 2026-11-05

- The production ARM64 AppImage built on Apple Silicon through Nix's Linux builder and ran in the disposable Ubuntu
  26.04 XFCE guest under QEMU/HVF, without a guest Nix store and with software rendering.
- Automatic MTP mounting, discovery, and inspection passed with Thunar closed. The initial run required mounting in
  Thunar; the rebuilt application resolved that failure.
- The Activities bookmark, FIT file listing, activity preview with map, and dismissal back to the usable file browser
  passed. The user completed the acceptance runner.
- File-dialog behavior was accepted and the running window's icon observed. Other picker behavior and desktop-menu
  integration are presumed working unless a concrete defect appears; they are not open qualification tasks.
- This establishes ARM64 desktop and USB/GVfs compatibility for this guest. Native GPU presentation was not exercised.

## Cross-GPU presentation failure

Forcing the AMD integrated GPU with no AMD-connected display failed DMA-buffer import on Wayland and surface creation on
XWayland. The Wayland failure reproduced outside the AppImage and with Homebrew's `vkcube --wsi wayland` forced to the
host Radeon ICD. This is a host graphics issue reproduced independently of the application.
