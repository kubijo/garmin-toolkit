---
level: error
---

# Keep postcard models structurally compatible

Postcard-backed models must not use serde representations that require self-description.

```grit
language rust

attribute_item() as $attribute where {
  $attribute <: contains `serde`,
  $attribute <: contains or {
    `tag`,
    `untagged`,
    `flatten`,
    `skip_serializing`,
    `skip_serializing_if`
  }
}
```

## Detects an internally tagged representation

```rust
#[serde(tag = "kind")]
enum WireMessage {
    Ready,
}
```

```rust
#[serde(tag = "kind")]
enum WireMessage {
    Ready,
}
```

## Allows a postcard-compatible representation

```rust
#[serde(rename_all = "snake_case")]
enum WireMessage {
    Ready,
}
```
