---
level: error
---

# Supply wall-clock dates from the host

Shared presentation code takes wall-clock input from its host. Gallery scenes and tests supply fixed dates. Keep
platform time reads in the application adapters; monotonic timing for animation, timeouts, and telemetry remains legal.
This profile covers `crates/garmin-ui/src`, not the desktop or browser hosts.

```grit
language rust

or {
  `jiff::Zoned::now()`,
  `Zoned::now()`,
  `jiff::Timestamp::now()`,
  `Timestamp::now()`,
  `std::time::SystemTime::now()`,
  `SystemTime::now()`,
  `chrono::Local::now()`,
  `Local::now()`,
  `chrono::Utc::now()`,
  `Utc::now()`,
  `js_sys::Date::now()`
}
```

## Detects a direct Jiff clock

```rust
let today = jiff::Zoned::now().date();
```

```rust
let today = jiff::Zoned::now().date();
```

## Detects an imported clock

```rust
let now = SystemTime::now();
```

```rust
let now = SystemTime::now();
```

## Detects timestamps

```rust
let now = jiff::Timestamp::now();
```

```rust
let now = jiff::Timestamp::now();
```

## Allows injected dates and monotonic timing

```rust
let today = props.today;
let frame = ui.input(|input| input.time);
let started = Instant::now();
```
