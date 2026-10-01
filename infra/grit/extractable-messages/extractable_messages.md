---
level: error
---

# Keep translated messages visible to extraction

The FormatJS Rust extractor does not enter other macros' token trees. Bind `format_message!` to a local before passing
it into a macro such as `vec!` or `assert!`. Catalog verification cannot detect messages that extraction missed.

```grit
language rust

identifier() as $name where {
  $name <: r"^format_message$",
  $name <: within token_tree()
}
```

## Detects a message in a vector

```rust
let rows = vec![(format_message!(intl, default_message: "Files to write"), count)];
```

```rust
let rows = vec![(format_message!(intl, default_message: "Files to write"), count)];
```

## Detects a qualified message inside a macro

```rust
assert_eq!(
    garmin_i18n::format_message!(intl, default_message: "Ready"),
    expected
);
```

```rust
assert_eq!(
    garmin_i18n::format_message!(intl, default_message: "Ready"),
    expected
);
```

## Detects a message nested inside another message

```rust
let text = format_message!(intl, default_message: "State: {state}", values: {
    state: format_message!(intl, default_message: "Ready")
});
```

```rust
let text = format_message!(intl, default_message: "State: {state}", values: {
    state: format_message!(intl, default_message: "Ready")
});
```

## Allows a local binding used in a macro

```rust
let label = format_message!(intl, default_message: "Files to write");
let rows = vec![(label, count)];
```

## Allows a message in a function call

```rust
ui.label(garmin_i18n::format_message!(intl, default_message: "Ready"));
```

## Allows an array converted to a vector without a macro

```rust
let rows = Vec::from([(format_message!(intl, default_message: "Files to write"), count)]);
```

## Allows message names in strings and comments

```rust
let example = stringify!("format_message!(intl, default_message: \"Ready\")");
// format_message! inside vec! is invisible to extraction.
let values = vec!["format_message"];
```
