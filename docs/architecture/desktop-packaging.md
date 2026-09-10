# Desktop packaging

AppImage and Flatpak target Linux `x86_64` and `aarch64`; only `x86_64` is hardware-tested. Production and demo have
separate application IDs and platform data:

| Mode       | Application ID                 | AppImage                                 |
| ---------- | ------------------------------ | ---------------------------------------- |
| Production | `io.kubijo.GarminToolkit`      | `dist/garmin-toolkit-ARCH.AppImage`      |
| Demo       | `io.kubijo.GarminToolkit.Demo` | `dist/garmin-toolkit-demo-ARCH.AppImage` |

Flatpak outputs use the application ID with a `.flatpak` suffix. Build them with
`just desktop::appimage production|demo` or `just desktop::flatpak production|demo`.

Both formats consume generated launcher metadata and brand assets. AppImage uses a pinned compatibility toolchain and
type-2 runtime. Flatpak builds offline from Nix-vendored Cargo sources on Freedesktop 25.08. Its current permissions are
development inputs, not a published security claim; the distribution plan requires proving the narrowest working USB and
GVfs permissions before release.
