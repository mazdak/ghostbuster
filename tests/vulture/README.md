# Vulture compatibility corpus

This directory contains small, language-neutral compatibility cases adapted
from Vulture's test suite. The upstream source and exact revision are recorded
in `UPSTREAM.toml`; Vulture's MIT license is copied in `LICENSE`.

The corpus is an evaluation input, not a copy of Vulture's Python API tests.
Vulture's tests usually inspect internal collections such as `unused_funcs` or
`unreachable_code`. Each case here instead describes
source files, analyzer options, and normalized diagnostics that a Rust test
runner can consume directly.

## Schema

`cases.json` has a top-level schema version, upstream provenance, and a `cases`
array. Every case contains:

- `id`: stable local identifier.
- `upstream_test`: Vulture test or tests from which the case was drawn.
- `files`: one or more relative paths and complete Python source strings.
- `parse_valid`: whether every source file is expected to parse. All initial
  cases are deliberately valid Python.
- `min_confidence`: optional inclusive confidence threshold; omitted means 0.
- `ignore_names`: optional Vulture-style name glob patterns.
- `ignore_decorators`: optional Vulture-style decorator glob patterns.
- `code_projection`: optional diagnostic codes to compare. When present, a
  runner filters actual findings to these codes before comparing them with
  `expected`. This keeps focused reachability cases from failing on unrelated
  unused symbols. When absent, `expected` is the complete result and any extra
  finding in any category fails the case.
- `expected`: ordered normalized findings after confidence filtering and code
  projection.

Each normalized finding has `path`, `start_line`, `end_line`, `kind`, `code`,
`name`, `message`, and `confidence`. Lines are one-based and inclusive. Expected
findings use Vulture's default ordering by case-folded path and start line.

The Rust harness requires at least eight unprojected cases whose complete expected
results collectively cover every supported finding code. This prevents a
cross-category regression from being hidden by the focused projections while
retaining those projections for cases adapted from category-specific upstream
tests.

## Codes and confidence

| Code | Kind | Confidence |
| --- | --- | ---: |
| V101 | attribute | 60 |
| V102 | class | 60 |
| V103 | function | 60 |
| V104 | import | 90 |
| V105 | method | 60 |
| V106 | property | 60 |
| V107 | variable | 60, or 100 for function arguments |
| V201 | unreachable_code | 100 |

## Oracle and interpretation

Expected diagnostics were generated with Vulture's typed `Vulture` API at the
pinned revision, using `scan()` for each listed file followed by
`get_unused_code()`. Every source was also parsed independently before it was
accepted into the corpus.

Vulture deliberately uses a global, scope-blind set of bare names. The
`cross-file-same-name-compatibility` case records that behavior: one use marks
both same-named definitions as used. A more precise Ghostbuster result may be a
desirable semantic improvement, but it should be reported as an intentional
compatibility difference rather than silently changing this corpus.

The upstream case
`tests/test_noqa.py::test_noqa_with_multiple_issue_codes` is excluded. Its
sample contains invalid Python but passes upstream because the test does not
assert the parse failure. CLI formatting, exit codes, automatic packaged
whitelists, and verbose logging are also outside this initial semantic corpus.
Packaged whitelists and CLI behavior have separate Rust regression tests.

## Suggested 25-case smoke set

For a fast first milestone, run these cases while preserving the full corpus
for the complete compatibility evaluation:

1. `unused-import`
2. `import-filtered-at-100`
3. `unused-class`
4. `unused-function`
5. `unused-method`
6. `unused-property`
7. `unused-variable`
8. `unused-attribute`
9. `unused-argument-at-100`
10. `all-marks-selected-import-used`
11. `init-module-imports-ignored`
12. `cross-file-function-used`
13. `getattr-string-marks-name-used`
14. `dunder-method-ignored`
15. `underscore-and-dunder-variables`
16. `test-functions-and-classes-ignored`
17. `ignore-decorated-function`
18. `ignore-decorated-class`
19. `flake8-noqa-codes`
20. `unreachable-after-return`
21. `unreachable-after-raise`
22. `unreachable-after-break`
23. `unreachable-after-continue`
24. `unsatisfiable-if`
25. `unsatisfiable-while`
