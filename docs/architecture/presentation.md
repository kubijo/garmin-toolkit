# Presentation foundations

`garmin-color` owns unpremultiplied encoded-sRGB `Color`: `0xRRGGBBAA`, `#RRGGBB`, and `#RRGGBBAA`. `palette` supplies
OKLab/OKLCH operations; `cint` is the lossless renderer boundary.

Themes derive from Carbon Gray 100 and Gray 10. Components use semantic roles, never swatch grades. Auto follows the
system and falls back to dark; an app-owned swatch supplies action states.

Profiles may store an accent and avatar. Import bounds content-detected PNG, JPEG, or WebP at 10 MiB and 4096 pixels per
edge, retains the original, and derives a 256-pixel PNG thumbnail. These fields affect presentation only.

`garmin-ui` owns component geometry, semantic color use, typed props and actions, and a curated Phosphor/local icon
catalog. Inputs inherit their surface layer. Modals may block the viewport or remain parent-contained; callers control
backdrop dismissal.

The UI embeds hinted Noto Sans 2.015 Regular and SemiBold. Named egui families carry weight because egui lacks a
font-weight field.

The activity browser lazily loads details and renders each continuous coordinate sequence through a map-ready path
preview. Online tiles remain separate work. Desktop runs application services on one worker thread and accepts native
selection or file drops. Demo recreates an isolated database and seeds it through the production importer.

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
resetting its scroll position. `infra/gallery/progress.capture.toml` covers concurrency, overflow, and completion in
both fonts at the supported terminal sizes.

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

`garmin-i18n` wraps FormatJS. English ICU messages live beside call sites as fallback; Czech source, translation, and
context compile into a flat catalog. Checks reject stale, missing, empty, extra, malformed, or incompatible messages.
That proves catalog completeness, not how much visible copy uses FormatJS. Gallery scenes expose both languages.
