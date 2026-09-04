---
level: error
---

# Keep self imports separate

Importing `self` alongside named items obscures which module name enters the local scope.

```grit
language rust

use_list() as $list where {
  $list <: contains self(),
  $list <: contains identifier()
}
```

## Detects a mixed self import

```rust
use crate::{self, DeviceManifest};
```

```rust
use crate::{self, DeviceManifest};
```

## Allows named imports without self

```rust
use crate::{DeviceManifest, TransportKind};
```
