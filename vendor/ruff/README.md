# Vendored Ruff crates

Ghostbuster directly uses Ruff's Python parser and AST instead of maintaining a second Python parser.

These five crates were generated with `cargo vendor --locked` from Ruff revision
`f34ad603f07c766f9c4941a2f23ae88c84812c93`. Ghostbuster uses versioned path dependencies, so the
vendored sources are selected regardless of the directory from which Cargo is launched. The exact
upstream revision is recorded in `REVISION`, the root README, and the third-party notice. Registry
dependencies continue to come from crates.io.

The normalized `ruff_python_ast` and `ruff_python_parser` manifests omit optional cache/schema
features and upstream-only development dependencies whose crates are not part of this deliberately
small runtime subset. Each crate has an empty workspace boundary so Cargo never promotes it to a
root workspace member. No Rust source was changed.

These path dependencies deliberately make the root package non-publishable. Build or install
Ghostbuster from a complete source checkout; a crates.io source archive would replace the paths
with unpublished Ruff-internal crate versions and is not a supported distribution format.

To refresh the copy, update `REVISION`, generate a locked vendor tree from that Ruff checkout,
replace the five crate directories here, repeat the manifest pruning described above, and regenerate
the root `Cargo.lock`. Keep the upstream `LICENSE` file with them.
