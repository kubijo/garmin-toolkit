# Visual language

Garmin Toolkit uses Carbon's semantic themes, spacing, and interaction model. Nautilus and File Pilot are observational
references; do not copy their assets, layouts, palettes, dimensions, or distinctive treatments. Start from shared
Toolkit primitives and roles.

## Foundation

- Gray 100/10 are the dark/light foundations. Surfaces follow Carbon layers; dark-theme children are not darker than
  parents. Components use `garmin-color` semantic roles, never swatch grades.
- Primary buttons use action blue; links, focus, progress, and active destinations use accent roles. Selection and open
  controls use neutral layers. Permanent panes use layers/dividers; shadows belong to menus, popovers, dialogs, toasts.
- Spacing: 2, 4, 8, 12, 16, 24, 32 logical pixels. Rows: 32; controls: 40; prominent controls: 48. Default desktop page
  inset: 24; dense workspaces: 12/16. Narrow layouts reduce insets deliberately.
- Controls, rows, cards, panes, menus, dialogs, and toasts are square. Radius tokens stay centralized; avatars/status
  circles remain circular. Compound controls may share edges. Select options form a flush list without gaps/insets.
- Hover, focus, selection, validation, disabled, and loading states preserve geometry. Validation changes the resting
  one-pixel border's color. Focus indicators do not move content.
- Noto Sans Regular/SemiBold: captions 12, body/controls 14, sections 16, page/dialog titles 20–24. Horizontal
  label/value pairs use equal size/weight and secondary label color; smaller labels belong above/below values.
- Icons come from the shared SVG catalog: 16 in controls, 20 in navigation, 24–32 for identity. No substitute text
  glyphs.
- Clickable targets use a pointer cursor; disabled calendar days use `not-allowed`.

## Composition

### Shell and navigation

Header: global identity, navigation toggle, profile, native controls. Page actions stay with their content. Navigation
uses compact full-width neutral selection, a leading full-height accent rail and accent icon. Narrow layouts retain
destination order and semantics. The navigation trailing edge is the shell divider; no header/content rule.

### Pages, cards, and forms

One page title, aligned content, optional contextual actions. Cards group objects or forms; avoid nested cards and
low-information full-width slabs. Form labels sit above filled/outlined controls, helper/error text below. Destructive
actions carry text. Modal actions dock to the trailing/bottom edges with 2-pixel gaps and shared footer spacing.
Empty/loading/error/offline states retain page context; blocking dialogs require a blocking decision or workflow.

### Dense workspaces

Use aligned columns, 32-pixel rows, concise metadata, and local toolbars. Left panes select/filter; right panes inspect
or act. Narrow layouts move subordinate panes into drawers/views without changing their role. Pointer, keyboard,
double-click, toolbar, and context-menu routes dispatch the same typed actions. Selection survives focus changes;
right-click is never the only action route.

### Activities

One scrollable workspace: calendar, summary, and actions beside the map on wide screens, above it on narrow screens.
That column ends at the map. Charts/laps span the full width below; chart summaries sit beside plots on wide views and
above on narrow views. Plot axes align; rows are compact. Summary rows have subtle dividers, including above Minimum.
Sample readouts wrap, show placeholders before selection, and retain the inspected sample after hover/scroll.

The calendar reserves six weeks; day labels are centered without trailing dots. Tiles have subtle fills in both themes
and 2-pixel gaps. Empty days are disabled. Month arrows browse months; activity arrows beside the title move across
populated days. A same-day list appears only for multiple activities. Selecting a new day selects its first activity;
selecting the current day preserves selection. **All activities** opens the searchable archive. Desktop import actions
belong with calendar navigation.

ICU4X owns calendar arithmetic, labels, and week rules; Jiff converts instants to local dates. Formatting retains
region, calendar, week-start, numbering, and hour-cycle preferences independently of UI language. Native Unix respects
`LC_ALL`/`LC_TIME`; other platforms/browsers use `sys-locale`. UI code caches month models and owns selection.

### Feedback

Inline feedback stays beside content; toasts report events; dialogs request decisions. Severity uses icons/words as well
as color. Animations are short, interruptible, and optional for comprehension. Toast surfaces/rails are square and clip
below native window controls, including shadows and animation. Maintained set: `notifications.capture.toml`.

## Verification

Inspect affected production scenes in both themes, narrow/full layouts, focus/hover/disabled/loading/error states, and
long/translated content. Captures live under `.tmp/gallery/`; maintained sets live in `infra/gallery/captures/`. See
[preview requirements](../decisions/0038-validated-interface-previews.md).

## References

Reviewed 2026-09-12: Carbon [themes](https://carbondesignsystem.com/elements/themes/overview/),
[spacing](https://carbondesignsystem.com/elements/spacing/overview/),
[grid](https://carbondesignsystem.com/elements/2x-grid/overview/),
[shell](https://carbondesignsystem.com/components/UI-shell-right-panel/usage/);
[GNOME principles](https://developer.gnome.org/hig/principles.html),
[utility panes](https://developer.gnome.org/hig/patterns/containers/utility-panes.html);
[File Pilot](https://filepilot.tech/).
