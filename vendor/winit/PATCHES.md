# Local changes to winit 0.30.13

Source: crates.io `winit` 0.30.13, upstream revision
`e9809ef54b18499bb4f2cac945719ecc2a61061b`. The published source and Apache-2.0 license are retained;
registry installation metadata `.cargo-ok` and the upstream lockfile are omitted.

## Wayland activation of an existing window

Upstream `Window::focus_window()` is a no-op on Wayland. The local backend implements it using
`xdg_activation_v1`, with the existing public winit and egui APIs unchanged:

- Pointer button, keyboard key, and touch down/up events capture the compositor serial, seat,
  originating top-level surface, and a weak reference to its window lifetime. Keyboard repeats and
  synthetic application events do not provide new activation input.
- All windows on the event loop share the latest input. A focus request consumes it once. Input older
  than five seconds, a closed originating window, or a destroyed seat/surface is rejected locally.
- The activation token uses the originating surface and its input serial; the token response activates
  the requested target surface. No environment variables or cross-process transport are involved.
- The asynchronous response checks the target window lifetime and request age before activation.
  Closing the window while a token is pending discards the activation. The token object is destroyed
  after its response in either case.
- The compositor makes the final focus decision. There is no retry or window recreation, and an
  unsupported compositor leaves focus unchanged.

The private activation lifetime regression test covers live, expired, closing, and dropped windows.
Actual activation requires a Wayland session and real input; see the acceptance checklist in
`docs/architecture/application-windows.md` at the repository root. Workspace QA does not run this
vendored crate's own unit tests.

Both workspace and gallery manifests patch winit to this directory. Nix includes the complete directory
in build and license sources. Remove the patch when an upstream release provides equivalent activation
from recent input on another application window.
