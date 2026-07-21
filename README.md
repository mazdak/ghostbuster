# Ghostbuster

[![CI](https://github.com/mazdak/ghostbuster/actions/workflows/ci.yml/badge.svg)](https://github.com/mazdak/ghostbuster/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/ghostbuster-code.svg)](https://pypi.org/project/ghostbuster-code/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Ghostbuster finds dead Python code without starting Python. It is a Rust command-line tool built
directly on Ruff's parser, syntax tree, source ranges, and line index.

The project takes Vulture's practical dead-code model as its behavioral reference while keeping a
small Rust-native interface and a slightly more precise distinction between normal names and object
members.

## Install

With a prebuilt binary from a tagged [GitHub release](https://github.com/mazdak/ghostbuster/releases),
or from PyPI under the `ghostbuster-code` distribution name:

```console
uv tool install ghostbuster-code
```

The PyPI name is longer because the unrelated `ghostbuster` name was already taken. Both methods
install the same `ghostbuster` executable and do not require Python at runtime. To build the full
source checkout instead:

```console
cargo install --git https://github.com/mazdak/ghostbuster --locked
```

## Try it

```console
cargo run -- src tests
```

Example output:

```text
src/example.py:8: V103 unused function 'retired_handler' (60% confidence)
src/example.py:13: V201 unreachable code after 'return' (100% confidence)
```

Useful options:

```console
ghostbuster --min-confidence 80 src
ghostbuster --exclude 'generated/**,vendor/**' .
ghostbuster --ignore-names 'visit_*,legacy_hook' src
ghostbuster --ignore-decorators '@app.route,@registry.*' src
ghostbuster --format json src > findings.json
```

Run `ghostbuster --help` for the complete interface. Explicit files are accepted regardless of
their extension. Directories are walked recursively for `.py` files while respecting Git and
standard ignore files.

Exit codes are designed for CI:

| Code | Meaning |
|---:|---|
| 0 | No dead code found |
| 1 | A path could not be read or Python could not be parsed |
| 2 | Invalid command-line arguments or patterns |
| 3 | Dead code found |

## Findings

| Code | Finding | Confidence |
|---|---|---:|
| V101 | Unused attribute | 60% |
| V102 | Unused class | 60% |
| V103 | Unused function | 60% |
| V104 | Unused import | 90% |
| V105 | Unused method | 60% |
| V106 | Unused property | 60% |
| V107 | Unused variable or argument | 60% / 100% |
| V201 | Unreachable code | 100% |

Ghostbuster understands definitions and loads across all input files in one run. It also handles
common dynamic-Python signals such as `__all__`, `getattr`, `hasattr`, decorators, properties,
dunder conventions, pytest entry points, type comments, dynamic format strings, Vulture's packaged
protocol whitelists, and `# noqa` comments. Source files support Python's UTF-8 BOM plus common
PEP 263 encodings, including Latin-1, Macintosh encodings, and numeric `cpNNN` code pages. `F401`
suppresses V104 and `F841` suppresses V107, so existing Ruff suppressions remain useful.

Reachability covers dead tails after `return`, `raise`, `break`, and `continue`; constant branches;
non-terminating `while` loops; terminal `if`, `try`, and exhaustive `match` branches; and nested
suites. Ruff's AST truthiness handles literals, with Vulture-compatible short-circuit evaluation
for constant `and`, `or`, and `not` expressions.

## Why Ruff rather than a new parser?

Python syntax evolves quickly and source positions are easy to get subtly wrong. Ghostbuster uses
the same Rust crates as Ruff and ty for parsing, AST traversal, UTF-8 byte ranges, Unicode columns,
CRLF lines, and BOM handling. The five required crates are checked into `vendor/ruff` and pinned to
one upstream Git revision, so builds do not silently switch parser implementations.

Ghostbuster itself only implements the dead-code policy that those projects do not expose as a
standalone public API. Pulling in ty's full database, module resolver, and type checker would make a
simple scanner much larger without replacing that policy layer.

## Vulture compatibility evaluation

The data-driven corpus in `tests/vulture` is derived from Vulture's upstream tests and pins the
exact source revision. It contains 46 cases: eight compare the complete result and collectively
cover V101–V107 plus V201; the other 38 project one diagnostic category so a focused reachability
test does not fail because an unrelated function in its snippet is also unused.

Ghostbuster deliberately preserves Vulture's project-wide bare-name behavior, which keeps the
compatibility oracle stable across modules. Its precision improvement is narrower: normal names
and object members live in separate namespaces, so a bare function call does not automatically
mark every same-named method alive. Intentional differences are documented separately from missing
compatibility behavior.

## Development

Ghostbuster pins the same Rust 1.97 toolchain currently used by Ruff. The required checks are:

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo check --workspace --all-targets --all-features --locked --offline
```

The minimum supported Rust version is 1.95. The pinned 1.97 toolchain supplies the formatter and
Clippy version used by CI. See [CONTRIBUTING.md](CONTRIBUTING.md) for the complete development
workflow.

- Ruff source revision: `f34ad603f07c766f9c4941a2f23ae88c84812c93`
- Vulture source revision: `2c21cb0ae2afa657e36f6a397cb573608a65d79e`

Ghostbuster is intentionally not a crates.io package: its Ruff internals are pinned path
dependencies. Build or install it from a complete source checkout, use the platform wheels on
PyPI, or download a native release archive.

## License

Ghostbuster is MIT licensed. Vendored Ruff code and Vulture-derived test data retain their upstream
MIT notices. See [THIRD_PARTY.md](THIRD_PARTY.md) for provenance and `THIRD_PARTY.html` for the
complete generated dependency notices shipped with releases.
