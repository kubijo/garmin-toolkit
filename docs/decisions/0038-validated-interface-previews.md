# 0038: Validated interface previews

## Decision

Every TUI, GUI, or web UI change requires a validated storybook preview in `infra/gallery` using its production renderer
and models. Scene compilation alone is insufficient: render captures and inspect the resulting images.

Cover loading, empty, success, error, cancellation, and recovery states where applicable. Inspect supported viewport
sizes, both TUI fonts, and supported languages. Include long device names, paths, URLs, and realistic error details.
Validate balanced prose wrap, no orphaned function words or symbols, unclipped content, contrast, focus, visible
controls, and clear completion. Exercise interaction and animation in the live preview.

## Evidence

Record scene IDs, revision, viewport, font, locale, and capture results with the change. Keep reproducible capture
recipes in the repository; rendered evidence belongs in local or CI artifacts. A reviewer must inspect images before
claiming UX validation. Runtime interaction tests cover behavior; exact-copy/style unit tests do not replace preview
evidence.

## Why

A passing build or a declared scene does not establish usable layout, readable prose, or a complete interaction.
