# Presentation foundations

`garmin-color` owns unpremultiplied encoded-sRGB `Color`: `0xRRGGBBAA`, `#RRGGBB`, and `#RRGGBBAA`. `palette` supplies
OKLab/OKLCH operations; `cint` is the lossless renderer boundary.

Themes derive from Carbon Gray 100 and Gray 10. Components use semantic roles, never swatch grades. Auto follows the
system and falls back to dark; an app-owned swatch supplies action states.

The application-wide [visual language](visual-language.md) defines the original Carbon-derived composition, geometry,
type, interaction, and verification rules. External applications are observational references only and are never copied.

Profiles may store an accent and avatar. Import bounds content-detected PNG, JPEG, or WebP at 10 MiB and 4096 pixels per
edge, retains the original, and derives a 256-pixel PNG thumbnail. These fields affect presentation only.

Profile settings uses an egui-elegance color-picker popover with internal preset swatches, recent colors, a continuous
selector, alpha slider, and hex entry. The profile circle previews the draft color immediately. Apply persists the
chosen RGBA color; reset restores the default. Both clients preserve the profile's other settings. Avatar markers
resolve the saved color against the current surface to maintain at least 3:1 contrast in both themes; text keeps
semantic colors.

Initials-only avatars blend the profile accent with the perceptual inverse of primary text for an opaque background. The
blend starts evenly and moves toward that inverse as needed to maintain at least 4.5:1 contrast with the primary-text
initials. Photo avatars retain their neutral backing.

`garmin-ui` owns component geometry, semantic color use, typed props and actions, and a curated Phosphor/local icon
catalog. Inputs inherit their surface layer. Modals may block the viewport or remain parent-contained; callers control
backdrop dismissal.

The UI embeds hinted Noto Sans 2.015 Regular and SemiBold. Named egui families carry weight because egui lacks a
font-weight field.

Use `typography::body(ui, text)` for explanatory prose: regular 14 px type, 20 px line height, secondary text color, and
balanced two-line wrapping when it fits. Longer paragraphs wrap naturally. The helper returns an egui response and keeps
its style local. For composition inside another widget, `typography::body_text(text)` returns `RichText` with the same
type metrics and inherits the widget's color. `typography::semibold(text)` selects the real semibold face for inline
emphasis. Callers own the spacing between paragraphs, headings, and controls.

The [activity workspace](activity-map.md) lazily loads recording details and links map, charts, laps, and playback
through one sample cursor. Online vector tiles use a bounded preparation and rendering pipeline. Desktop runs
application services on a worker thread and accepts native selection or file drops. Database jobs and results carry a
deployment epoch; restore discards results from the old database. Demo seeds an isolated deployment once through the
production importer and preserves it across restarts. The [device explorer](device-explorer.md) reuses the viewer for
FIT previews.

The borderless shell owns drag space and window controls; the compositor moves and resizes. Close and `Ctrl+Q` exit;
`Ctrl+W` does nothing. Active work is named before aborting, and shutdown waits between atomic imports.

One icon declaration produces typed constants and `ALL`. Build-time validation checks SVG structure, filled monochrome
geometry, names, and duplicates before normalizing assets for tinting. Local SVGs use the same catalog after stroke
outlining.

`infra/gallery` is an isolated `rs-gallery` workspace. Scenes live beside components and depend only on local props.
Captures cover states, sizes, layouts, flows, and the searchable icon catalog.

Desktop and HASS use the same per-storage capacity component. The native HASS host serves an egui/WASM client and a
streaming download loader; browser code receives owned models through the typed service boundary.

[ADR 0038](../decisions/0038-validated-interface-previews.md) requires rendered and inspected preview evidence for every
interface change, including prose wrapping and interaction. Scene compilation alone does not satisfy that gate.

CLI progress separates stage totals from identity-keyed file events. Active files retain arrival order across stages;
their capped panel scrolls independently of history. Diagnostic changes and completed work enter history without
resetting its scroll position. `infra/gallery/captures/progress.capture.toml` covers concurrency, overflow, and
completion in both fonts at the supported terminal sizes.

Every blocking CLI read uses the production `LoadingScreen` component with a typed activity. Discovery, device metadata,
recovery, storage, and map-component loading share its layout, continuously increasing elapsed clock, and cancellation
behavior. Path-like details such as `GarminDevice.xml` use the same file styling as the rest of the TUI. Gallery scenes
render the production component rather than duplicating it.

CLI gallery fixtures live in `garmin_cli_tui::preview`, behind one module-level feature gate. Rendering and terminal
input remain shared with production. Each font preview retains its own scroll state across animation and resizing; wheel
input targets the hovered panel, and clicking enables keyboard control. Tab switches panels; Escape releases gallery
focus.

Image baselines are deferred. Until capture coverage can merge into LLVM profiles, the numeric gate excludes `garmin-ui`
and desktop views while their tests still run.

The Web canvas intentionally owns secondary-click interaction instead of opening the browser's generic context menu.
Application context menus expose only actions meaningful for the item or surface under the pointer and dispatch the same
typed actions as the visible interface; right-click is never the sole route to an operation. Keyboard users can open the
same menu with the platform context-menu key or `Shift+F10`. Native window-titlebar secondary-click behavior remains
owned by the operating system where supported.

File browsers and server file choosers hide dot-prefixed entries in both the tree and file list by default. The
toolbar's Show/Hide hidden files toggle saves to the active profile and follows it across windows and restarts. It is a
display preference, not an access restriction.

`garmin-i18n` wraps FormatJS. English ICU messages live beside call sites as fallback; Czech source, translation, and
context in `translations/cs.json` compile into a flat catalog in Cargo's `OUT_DIR` using the pinned FormatJS CLI during
the crate build. Generated catalogs are not tracked. `just dev::i18n-sync` updates editable translation metadata; checks
reject stale, missing, empty, extra, malformed, or incompatible messages. That proves catalog completeness, not how much
visible copy uses FormatJS. Gallery scenes expose both languages.
