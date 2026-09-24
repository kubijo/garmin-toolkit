# Visual language

Garmin Toolkit uses an application-owned visual language built on the semantic theme, spacing, type, and interaction
models of Carbon. It is not a Nautilus skin, a File Pilot clone, or an attempt to reproduce either product.

## Originality boundary

Nautilus and File Pilot are observational references only. They help name qualities that this application also needs:
calm chrome, clear hierarchy, efficient navigation, dense-but-readable work areas, direct manipulation, and complete
mouse and keyboard access. Do not copy their layouts, assets, palettes, exact dimensions, component silhouettes, or
signature treatments. A screen must still make sense when described only in Garmin Toolkit and Carbon terms.

When a reference and Carbon point in different directions, follow Carbon unless an application-specific requirement is
recorded here. New components start from existing Garmin Toolkit primitives and semantic roles. They do not introduce
one-off colors or geometry in order to resemble a screenshot.

## Foundation

### Color and elevation

- Keep Carbon Gray 100 and Gray 10 as the dark and light foundations. Surfaces use Carbon's semantic layering model:
  global background, then progressively differentiated layers. Do not make child surfaces darker than their parent in
  the dark theme.
- Use color roles, never swatch grades, in components. Background, layer, field, hover, selected, border, content,
  support, and action colors remain renderer-neutral roles owned by `garmin-color`.
- Filled action blue is reserved for primary CTA surfaces. Links, focus, progress, and active-destination indicators may
  use the semantic accent without flooding their containing surface. Open controls and selected/highlighted rows use
  subtle neutral layers rather than borrowing primary-action fill.
- Use borders to clarify controls and discrete cards, not to box every region. Adjacent workspace panes use semantic
  layer color without strokes; a strong border marks editable controls; the focus color is reserved for a visible
  indicator that does not change geometry.
- Shadows belong only to transient surfaces above the workspace: menus, popovers, dialogs, and toasts. Permanent panes
  express hierarchy through layer color and dividers.

### Geometry and spacing

- Use Carbon's 2× rhythm. The working spacing sequence is 2, 4, 8, 12, 16, 24, and 32 logical pixels.
- Compact information rows are 32 pixels high; ordinary controls are 40; prominent controls are 48. Exceptions need a
  content or accessibility reason, not a local aesthetic preference.
- Ordinary controls, rows, cards, panels, menus, dialogs, drop zones, and floating surfaces use zero radius. Retain the
  named control, panel, and floating-surface tokens so the rule stays centralized. Rounding is reserved for genuinely
  circular identity and status containers such as avatars; it is not general surface decoration. Adjacent controls may
  share an edge only when they act as one compound control.
- Select triggers, their popup surfaces, and option rows use zero radius at every size. Popup choices form one flush
  list: no inner margin surrounds the rows and no gap separates adjacent choices.
- Interaction state must never change geometry. Rest, hover, active, focus, selected, disabled, loading, and error
  states preserve bounds and surrounding layout.
- Default page padding is 24 pixels at desktop widths and reduces deliberately at narrow widths. Dense workspaces may
  use 12 or 16 pixels internally while retaining clear pane boundaries.

### Typography and icons

- Noto Sans Regular and SemiBold remain the application faces. Use a small role set: 12-pixel caption/helper, 14-pixel
  body and control label, 16-pixel section heading, and 20–24-pixel page or dialog title.
- Prefer size, weight, and spacing over extra colors. Body text is regular; control labels and compact headings may be
  semibold; large display type is exceptional.
- Titles describe the current place or task. Supporting copy is short, plain, and normal weight. Do not put transport
  diagnostics into headings.
- Every interface symbol comes from the shared SVG catalog. Text glyphs are never substitutes for carets, folders,
  storage, status, or actions. Standard sizes are 16 pixels in controls, 20 in navigation, and 24–32 for object
  identity.

## Application composition

### Shell and navigation

- The shell is stable orientation, not a dashboard frame. Its header contains only global identity, navigation toggle,
  profile, and native window controls. Page actions belong to the page header or local toolbar.
- The navigation pane is a quiet first layer over the application background. Destinations use compact, full-width
  selection surfaces; a full-height accent rail begins at the pane's leading edge, and the accent icon reinforces the
  active destination without turning the row into a primary action.
- Navigation groups use sentence case and modest separation. Device names and state are content, never decorative
  badges.
- At narrow widths, preserve the same destination order and semantics while collapsing labels. Do not create a second
  mobile-only information architecture.
- Header and page content meet without a horizontal rule. The navigation pane's trailing edge is the shell's only
  internal divider.

### Pages, cards, and forms

- Each page has one clear title and an optional contextual action group. Content aligns to a consistent leading edge and
  uses whitespace to separate major sections.
- Use cards only for a meaningful grouped object, summary, or bounded form. Avoid nested cards and full-width slabs that
  merely tint empty space.
- Form labels sit above controls. Editable fields are filled and outlined; helper and error text sit below without
  moving the control. Primary actions are obvious but scarce, and destructive actions always carry text.
- Validation changes the field border's semantic color and supplies an error message without increasing the resting
  one-pixel border width.
- Modal action groups dock to the dialog's trailing and bottom edges. Adjacent actions have a two-pixel gap, and the
  shared body-to-footer spacing stays compact; individual dialogs do not override this geometry.
- Empty, loading, failed, and offline states preserve the page context. Blocking dialogs are for decisions or workflows
  that truly prevent safe continuation, not ordinary navigation.

### Dense workspaces

- Browsers, activity lists, and inspectors may be denser than settings pages. Density comes from consistent 32-pixel
  rows, aligned columns, concise metadata, and local toolbars—not tiny text or ambiguous icons.
- A left pane changes or filters the main view. A right pane explains or acts on the current selection. On narrow views,
  subordinate panes may overlay or become a separate view without changing their semantic role.
- Selection, double-click, visible toolbar actions, keyboard commands, and contextual menus dispatch one typed action
  model. Right-click is an additional route, never the only route.
- Tables and trees support pointer and keyboard traversal. Hover uses a stable layer fill; selection remains visible
  when focus moves to a details pane.

### Feedback

- Inline feedback stays near the affected content. Toasts report completed or externally triggered events and may carry
  one concise action. Dialogs request decisions. The offline surface blocks only while the application truly cannot
  continue.
- Severity is communicated by icon and words as well as color. Animations are short, interruptible, and never required
  to understand state. Reduced-motion behavior must remain possible.
- Toast surfaces and their full-height severity rails are square so stacked edges remain exact.
- Desktop toast bounds exclude the shell header. Entry animations, shadows, and expanded stacks stay clipped below the
  window controls, including after resizing. `infra/gallery/notifications.capture.toml` maintains this evidence.

## Verification

Every shared primitive and application page needs dark, light, narrow, keyboard-focus, hover, disabled, loading, and
error evidence as applicable. Gallery captures are reviewed as a system: adjacent scenes should visibly share spacing,
surface, typography, focus, and icon rules. A local polish change is incomplete if it introduces an exception that is
not represented by a semantic token or component variant.

All maintained captures and temporary review renders live beneath the repository's `.tmp/gallery/` directory. Do not
scatter gallery evidence through the system temporary directory.

## Reference record

The synthesis was reviewed against these sources on 2026-09-12:

- [Carbon themes](https://carbondesignsystem.com/elements/themes/overview/),
  [spacing](https://carbondesignsystem.com/elements/spacing/overview/), and
  [2× Grid](https://carbondesignsystem.com/elements/2x-grid/overview/) define the implementation foundation.
- [Carbon UI shell](https://carbondesignsystem.com/components/UI-shell-right-panel/usage/) and its
  [semantic states](https://carbondesignsystem.com/components/UI-shell-right-panel/style/) inform persistent shell and
  pane behavior.
- [GNOME design principles](https://developer.gnome.org/hig/principles.html),
  [header bars](https://developer.gnome.org/hig/patterns/containers/header-bars.html),
  [utility panes](https://developer.gnome.org/hig/patterns/containers/utility-panes.html), and a current
  [Ubuntu 24.04 Nautilus dark screenshot](https://commons.wikimedia.org/wiki/File:Ubuntu_24.04_LTS_Nautilus_46.0_dark_theme_-_English.png)
  were inspected for contemporary Linux conventions.
- File Pilot's [official overview](https://filepilot.tech/) and
  [Handmade Network project page](https://filepilot.handmade.network/) were inspected for workspace capabilities,
  information density, and mouse/keyboard parity—not for visual reproduction.
