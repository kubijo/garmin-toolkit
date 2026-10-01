---
level: error
---

# Preserve intentional message layout

FormatJS collapses whitespace in `default_message` before extraction and rendering. Use a value such as `{br}` for a
deliberate line break. This checks message text, not descriptions or interpolation values.

```grit
language rust

or {
  string_literal() as $message where { $message <: r"(?s).*?(?:\\[nt]|\n|\t| {2,}).*" },
  raw_string_literal() as $message where { $message <: r"(?s).*?(?:\n|\t| {2,}).*" }
} where {
  $message <: after `default_message`,
  $message <: within macro_invocation()
}
```

## Detects escaped line breaks

```rust
let text = format_message!(intl, default_message: "Done.\n\nDevice unchanged.");
```

```rust
let text = format_message!(intl, default_message: "Done.\n\nDevice unchanged.");
```

## Detects doubled spaces

```rust
let text = format_message!(intl, default_message: "Done.  Device unchanged.");
```

```rust
let text = format_message!(intl, default_message: "Done.  Device unchanged.");
```

## Detects a multiline raw message

```rust
let text = format_message!(intl, default_message: r#"Done.
Device unchanged."#);
```

```rust
let text = format_message!(intl, default_message: r#"Done.
Device unchanged."#);
```

## Allows deliberate breaks and descriptive whitespace

```rust
let text = format_message!(intl, default_message: "Done.{br}Device unchanged.",
    description: "Completion.  Preserve the break.", values: { br: "\n\n" });
```

## Allows literal backslash letters in raw strings

```rust
let text = format_message!(intl, default_message: r"Use \n for a newline.");
```
