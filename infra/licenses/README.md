# License bundles

`just qa::licenses` checks the distributable desktop and HASS notices. `just in-shell nix run .#licenses` regenerates
them. Generation selects the normal dependency closure for each shipped component, including the HASS web client.

`cargo-bundle-licenses` supplies license identifiers and candidate texts. Its parent-directory search is unbounded; the
generator only accepts candidate texts that occur in notice files inside the corresponding package. This keeps the
application's own notices out of dependencies when Cargo home lives under this checkout. Package paths and symlinks
cannot escape that boundary. File bytes retain upstream line endings.

`notices.toml` supplies exact package versions and license identifiers when the harvester cannot discover a notice:

- `package_file` names a notice shipped inside the crate, including nonstandard filenames and nested directories.
- `file` names an upstream copy under `notices/`, with its SHA-256 and a source URL pinned to the published crate's VCS
  revision. Identical copies share storage; each source revision remains recorded.
- `include_standard_terms` appends pinned full terms to notices that provide only a grant or a terms URL, preserving the
  original notice and its copyright attribution.

These copies retain upstream formatting and are checked by hash rather than reformatted. The manifest, Python, Nix, and
documentation remain enrolled in repository QA. When updating a dependency, review the corresponding supplements against
its new source revision; stale package versions fail instead of silently reusing old notices. For upstream files without
a final newline, `no_final_newline` removes the storage newline before checking the upstream hash and bundling the
original text.

For reviewed packages that declare standard SPDX licenses without a separate notice file, `declarations` records the
exact version, expression, original manifest hash, and upstream manifest URL. Generation verifies that the published
manifest, Cargo metadata, repository, and VCS revision agree. It includes each declared alternative with checked-in,
pinned SPDX `terms`, and labels author attribution as Cargo metadata. The standard MIT and BSD copyright example lines
are omitted using exact, reviewed template adjustments; no copyright owner or year is inferred. Actual notices shipped
inside a package take precedence over a declaration fallback. A verified supplement may extend a shipped notice only
when it preserves that notice verbatim as its prefix.

These records cover single licenses and explicit OR alternatives. Other expressions require a separate review;
generation never flattens AND requirements or license exceptions. New packages do not automatically receive a fallback,
and changed declarations, source hashes, or alternatives fail the check.

Each notice requires nonempty text; literal `NOT FOUND` is an error even when another license for the package has text.
Every target is validated before any output is replaced. The Nix generator closure includes the Python modules,
manifest, and notice texts; generation and checking need no network access after Cargo sources are available.
