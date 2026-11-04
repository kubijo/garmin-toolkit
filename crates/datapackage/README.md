# Data Package

An application-independent Rust reader/writer for local Frictionless Data Packages. There are no Garmin models,
database, async runtime, CLI application, Python runtime, or repository tooling dependencies in the library.

The current scope is v1/v2 descriptors, local paths, inline row arrays, ZIP transport, SHA-256/byte integrity, CSV
string/integer/number/boolean/any fields, required values, and primary/foreign keys. Metadata extensions survive round
trips. Network resources, external schemas, custom dialects, other field types, and additional constraints are outside
the supported validation subset; they are not silently certified as valid. Reading and validation are separate. Numeric
conversion uses signed 128-bit integers and finite `f64` numbers.

Garmin's activity profile, units, original FIT policy, and persistence remain in the application crates.

## Tests

```bash
cargo test -p datapackage
cargo build -p datapackage --example conformance
DATAPACKAGE_READER="$PWD/.tmp/cargo-target/debug/examples/conformance" \
    uv run --frozen --project crates/datapackage/compat \
    python -m unittest discover -s crates/datapackage/compat -v
```

`tests/upstream/manifest.json` pins actual upstream data, a pristine Python test reference, their release commit, and
SHA-256 hashes. The executable test copy may have local comment formatting; its AST must match the pinned reference. The
compatibility adapter takes literal inline descriptors from those tests using Python's AST, compares Rust/Python
validity and issue codes, and requires the official validator to accept Rust's rewritten packages. No network access or
external services are needed after dependencies are installed.

The vendored upstream material retains its MIT license in `tests/upstream/LICENSE.md`. The crate retains this
repository's AGPL-3.0-or-later license.

## Extraction

Source, license, fixtures, examples, and compatibility tooling live inside this crate. Only published registry
dependencies are used. Cargo's package archive materializes inherited manifest values into an independent manifest:

```bash
cargo package -p datapackage --allow-dirty --no-verify
```

That archive can be unpacked into another repository and built without the application workspace. The compatibility
command there uses `uv run --frozen --project compat` and the standalone target's `target/debug/examples/conformance`
binary. The compatibility project owns its `pyproject.toml` and `uv.lock`.
