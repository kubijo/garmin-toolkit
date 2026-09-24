# Application windows

`garmin_ui::window::WindowHost` is the shared contract for Developer tools and device files. Consumers supply a stable
logical ID, content kind, title, initial size, typed snapshot, renderer, and typed commands. Hosts own opening/focusing,
closing, delivery, and acknowledgements. They do not own automation, device operations, or backend storage.

There are two implementations:

- `NativeWindow<C, S>` renders through egui's immediate secondary viewport API. This lets the consumer borrow its
  existing controller state and receive actions in the same frame. Snapshots are unused. Window close and action events
  return to the owning controller. Reopening an existing logical ID focuses it without resetting its size or content.
  Native child windows disable platform decorations and reuse the application's title-bar controls, drag behavior, and
  resize edges. A theme-aware border and a shadow on a transparent surround separate overlapping windows; maximizing
  removes the shadow surround. Content is clipped inside the border. Close returns to the child controller; it does not
  close the main app. Embedded fallback views keep egui's own window frame.
- HASS `BrowserWindow<C, S>` opens an independent Rust/egui runner through `web-sys`. The same application URL,
  including an ingress prefix, carries `app-window=<random-session>&window-kind=<kind>`. The startup router selects the
  consumer before normal map/app initialization. Repeated opening focuses the same browser window; reopening a closed
  window creates a fresh session. Blocked popups offer an explicit retry or tab fallback.

The browser transport and popup client are shared by both consumers. A bounded, session-specific BroadcastChannel
carries typed `Message<C, S>` envelopes: poll, snapshot, command, and acknowledgement. Popups poll every 500 ms and stop
admitting commands after five seconds without a response. Only one command awaits acknowledgement at a time; a timeout
does not retry it. Snapshots are constructed only in response to polling, at most once per host frame. The channel
accepts at most 64 queued messages and 32 MiB per serialized message; oversized outgoing snapshots report an error.
Closing or reloading the parent cannot connect an old popup to a different app document.

New window kinds register a typed Rust consumer and runner. Browser APIs, channels, liveness, and popup fallback stay in
the platform implementation.

## Native Wayland focus

The shared native host sends `ViewportCommand::Focus` when an existing window is opened again. The local
[winit patch](../../vendor/winit/PATCHES.md) implements that command's Wayland backend using `xdg_activation_v1`. Real
pointer, keyboard, or touch input supplies the initiating window, seat, and compositor serial. A focus request consumes
that input once, requests a token for the initiating surface, and activates the child surface when the token arrives.
Input expires after five seconds; closed source windows and closed targets are rejected. The compositor decides whether
to grant focus.

Application controllers do not handle activation tokens. Both Developer tools and device files use the existing host
API, and focusing preserves each window's identity, size, and state. Synthetic egui automation cannot supply compositor
input serials, so compositor focus acceptance requires real input.

Upstreaming and an explicit-token alternative are tracked in the
[shared interface plan](../plans/shared-interface-workflows.md#deferred-wayland-activation-work).

## Consumers

Developer tools renders the same panel on both platforms. Native commands go to the root automation driver; browser
commands go to the originating app tab. Logs use the existing log service directly from the tools runner. Automation
continues when tools is closed, and only the root viewport contributes semantic targets to its driver.

Device files keeps one active device window per app, bound to the profile that opened it. Logout, profile removal, or
switching to another profile closes the file window and its FIT preview on both platforms and clears their cached views.
Late browser catalogue and operation results are discarded after that scope ends. Developer tools is app-wide and stays
open. Ordinary navigation and resizing do not end the profile scope. The window identity includes the device key. The
native window borrows the existing `device_browser::Browser`; worker operations remain owned by the desktop application.
Browser files has its own explorer view, service connection, and upload picker. The popup invokes the existing file RPC
controllers for upload, download, folder creation, and removal. These operations retain their backend path validation,
transfer limits, confirmation, and connection-generation guards. Picker admission reserves the operation before awaiting
file selection; cancellation, reconnection, or loss of the parent session invalidates that reservation.

Browser FIT Open and Import commands return to the parent app, where the existing preview/import flows live. Admission
checks the device key, selected profile, connection generation, catalogue availability, and current busy state. A local
mutation refreshes the popup catalogue and requests a parent catalogue reload. Repeated identical snapshots do not reset
directory, selection, history, or drawers. Opening files requests a fresh catalogue unless a parent-side FIT operation
is still active. Theme and language follow the parent; diagnostics use the same centralized log forwarding.

Closing a native file window leaves its worker operation in the app. Closing a browser file window disposes its local
view and transport; requests already admitted by the backend may have taken effect. Writes are never replayed on reopen
or reconnect. File contents are not sent through the window channel.

### File-window requests during background operations

Desktop can open or focus the current user's existing device-file view while its catalogue or transfer is busy. That
request does not enqueue more device work; switching device identities waits for the active operation to finish. HASS
likewise allows opening or focusing the same owner's device-file popup during parent-side FIT Open/Import, without
requesting another catalogue; switching devices remains blocked during that operation. Browser catalogue requests
supersede requests for a different device. Only the latest request can publish a catalogue or clear its loading
indicator, and logout/disconnect invalidates pending completions.

Runtime checks are tracked in the [shared interface plan](../plans/shared-interface-workflows.md#runtime-acceptance).
