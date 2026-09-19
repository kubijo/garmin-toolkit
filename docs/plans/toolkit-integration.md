# Cross-cutting integration gates

Apply these gates while closing each working slice. Finish
[single-path execution](../decisions/0037-single-execution-path.md). Before the first release, change development-only
state formats directly rather than adding compatibility branches.

## Remaining work

1. Verify packaged production/demo IDs, data roots, and binary names. Exercise desktop startup with a populated database
   and HASS environment precedence through the built applications.
2. Measure device/update/CLI coverage after the completed
   [mounted-update work](../research/usb-sync.md#map-maintenance-evidence). Raise the enforced floor through behavioral
   coverage, without blanket exclusions or suppression.
3. Audit crate boundaries after the transaction work. Keep a crate only for an independently reusable capability or a
   dependency-inversion boundary. In particular, keep `garmin-progress` limited to operation observation and
   cancellation; move policy, persistence, and presentation to their owners.
4. Measure translation adoption separately from catalog completeness. Inventory production UI surfaces, route their
   user-visible prose through typed FormatJS descriptors, and reject unregistered copy mechanically. Review English
   source prose before translating it; do not preserve poor wording merely to keep a catalog ID stable.

### Translation tooling: pseudo-locale coverage

Use synthetic locale `en-XA` to expose translation bypasses and layout assumptions. Carry forward these technical
lessons when implementing the translation-adoption gate:

- Generate pseudo catalogs through the existing FormatJS extractor/compiler (`compile --ast --pseudo-locale en-XA`), not
  a second ICU parser or a transform over already formatted messages. Preserve interpolation values and plural
  structure. Keep locale metadata in one shared descriptor and reject conflicting messages under the same ID when
  merging surfaces.
- Treat pseudo mode as a non-persisted testing override, not a production language choice. Test entering and leaving it
  even when the underlying language remains English; cached formatters and labels created outside rendering can
  otherwise retain stale text.
- Account for locale-generated dates outside message catalogs and verify the actual font stack shapes every pseudo
  letter without missing glyphs. Otherwise ordinary date output or missing-glyph boxes can masquerade as untranslated
  copy or clipping.
- Inspect expanded text for fixed-English-width layouts, character-budget truncation, clipped brackets, and missing
  wrapping space. Keep deliberate verbatim values distinct from untranslated prose; inspect reasons and captions
  crossing API boundaries too.
- Cover native/gallery and browser surfaces, including overlays and interaction states, with capture dimensions that
  actually include the content. Capture tall gallery catalogs separately and browser pages at desktop/mobile widths.
  English-versus-pseudo image differences are expected, but capture/build failures must remain failures. Pin the
  headless software renderer and browser/driver pairing through Nix.
- Prove checks fail on deliberately untranslated copy and clipped text before trusting green results. Guessed DOM
  selectors can produce false positives and false negatives; checks must not depend on warnings omitted from production
  builds. For this egui app, use semantic/rendering evidence rather than assuming DOM text scraping covers canvas
  content.

Evidence boundary: screenshots support inspection but do not assert translation coverage. Image-comparison failures are
not counts of translation defects, and successful captures do not prove coverage. Keep catalog parity, observed UI
coverage, layout review, and the future mechanical bypass gate separate; do not close this item on successful capture
alone.

## Exit criteria

- Current state and persisted plans use one versioned format without pre-release compatibility branches.
- Built identities, sandboxed checks, security audit, and behavioral coverage have revision-specific evidence.

Hardware proof: [USB synchronization](../research/usb-sync.md#map-maintenance-evidence). Capacity discovery:
[device state](device-state.md). Delete this plan once its remaining contracts and evidence have durable homes.
