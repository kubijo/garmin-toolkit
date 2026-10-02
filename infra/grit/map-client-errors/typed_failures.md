---
level: error
---

# Keep map request failures typed until presentation

The desktop and browser map adapters must retain `Failure` in `ClientError::Request`. Extracting its English `message`
loses the kind needed for localization. The shared presenter translates the kind using the current client language and
offers the original diagnostic in Details. This deliberately covers the two map adapters, not unrelated diagnostic data.

```grit
language rust

or {
  field_expression(field=$field),
  field_pattern(name=$field)
} where {
  $filename <: r"(?:.*/)?maps\.rs$",
  $field <: r"^message$"
}
```

## Detects a diagnostic forwarded as a user-facing error

```rust
// @filename: maps.rs
state.error = reply.err().map(|failure| failure.message);
```

```rust
// @filename: maps.rs
state.error = reply.err().map(|failure| failure.message);
```

## Detects destructuring that discards the typed failure

```rust
// @filename: maps.rs
let Failure { message, .. } = failure;
```

```rust
// @filename: maps.rs
let Failure { message, .. } = failure;
```

## Allows retaining the typed failure

```rust
// @filename: maps.rs
state.error = reply.err().map(ClientError::Request);
```

## Allows connection diagnostics in their own variant

```rust
// @filename: maps.rs
state.error = Some(ClientError::Connection(error.to_string()));
```

## Leaves unrelated diagnostic consumers outside this migration

```rust
// @filename: logging.rs
let text = event.message;
```
