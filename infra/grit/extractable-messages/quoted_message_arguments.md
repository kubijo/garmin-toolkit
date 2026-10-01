---
level: error
---

# Keep message arguments active

An ASCII apostrophe before `{` starts ICU quoting and can render the placeholder literally. Reword the message or use
typographic quotation marks. Catalog verification alone cannot catch an accidentally quoted argument that is also used
elsewhere in the message.

```grit
language rust

or { string_literal(), raw_string_literal() } as $message where {
  $message <: after `default_message`,
  $message <: within macro_invocation(),
  $message <: r"(?s).*'\{.*"
}
```

## Detects a quoted argument even when another occurrence is active

```rust
let text = format_message!(intl, default_message: "State '{state}': {state}", values: { state });
```

```rust
let text = format_message!(intl, default_message: "State '{state}': {state}", values: { state });
```

## Detects raw strings

```rust
let text = format_message!(intl, default_message: r#"State '{state}'"#, values: { state });
```

```rust
let text = format_message!(intl, default_message: r#"State '{state}'"#, values: { state });
```

## Allows typographic quotes and ordinary apostrophes

```rust
let text = format_message!(intl, default_message: "Device's state: ‘{state}’", values: { state });
```

## Allows examples in descriptions and values

```rust
let text = format_message!(intl, default_message: "State: {state}",
    description: "Avoid '{state}' in the message", values: { state: "'{ready}'" });
```
