# Contributing to Ghostbuster

Thanks for helping make dead-code analysis faster and more accurate.

## Before opening a change

- Search existing issues and pull requests.
- Keep behavior changes small and explain whether they match Vulture or intentionally differ.
- Do not replace the vendored Ruff parser with a second Python parser.
- Add a focused regression test for every bug fix.

## Development setup

Install the toolchain pinned in `rust-toolchain.toml`, then run:

```console
cargo fetch --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
cargo check --workspace --all-targets --all-features --locked --offline
```

CI also verifies Rust 1.95, dependency advisories, approved licenses, and the Python wheel. Use
`uvx maturin build --release --locked --out dist` to exercise the wheel locally.

## Compatibility changes

The cases in `tests/vulture/cases.json` are pinned to one Vulture revision. Update expectations only
after checking the same source with that pinned Vulture checkout. Full-result cases must continue
to cover V101–V107 and V201.

Vendored Ruff updates should be isolated changes. Record the new commit in `vendor/ruff/REVISION`,
`THIRD_PARTY.md`, the corpus documentation where relevant, and regenerate `Cargo.lock` and
`THIRD_PARTY.html`.

## Pull requests

Use a conventional title such as `fix(analyzer): reject malformed type comments`. Describe the
user-visible behavior, tests run, and compatibility impact. By contributing, you agree that your
work is licensed under the repository's MIT license.
