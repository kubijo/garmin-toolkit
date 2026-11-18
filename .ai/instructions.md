# Repository instructions

Markdown tables must fit within 120 source columns, including padding and pipe characters, without wrapping rows or
cells. Shorten columns, move explanations outside the table, or use prose or lists when a table cannot fit. Check the
width after formatting.

Use an 8 px spacing scale: multiples of 8 px, with 4 px or 2 px for compact gaps.

Use typed quantities throughout code we control, including calculations, snapshots, digest inputs and formatters.
Convert to raw numbers at external interfaces that require them. Prefer serialization on the type when it can preserve
the required wire format. Use `std::time::Duration` as the design reference for quantity types: unit-named constructors
and accessors, an exact internal representation where possible, and arithmetic with explicit overflow behavior. Put unit
conversions and formatting on the quantity type.

Keep gallery `.scene.rs` files beside the implementation they demonstrate, using the same filename stem when applicable.
`infra/gallery` owns the host, discovery configuration, and capture recipes, not component scenes.

Use the edit tool for file changes. Do not create or rewrite content through shell/Python heredocs.

In Nix, inherit used `builtins` attributes at the top of the scope instead of calling `builtins.name`.

Keep CI YAML declarative. Put orchestration and branching in checked scripts; pass parameters from the workflow.
