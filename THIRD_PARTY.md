# Third-party material

The release bundles also contain `THIRD_PARTY.html`, generated from the locked Cargo graph with
`cargo-about`. That file carries the dependency license texts required for binary redistribution.
Regenerate it after dependency changes with:

```console
cargo about generate about.hbs > THIRD_PARTY.html
```

## Ruff

- Project: <https://github.com/astral-sh/ruff>
- Revision: `f34ad603f07c766f9c4941a2f23ae88c84812c93`
- License: MIT
- Local material: `vendor/ruff`

Ghostbuster vendors `ruff_python_parser`, `ruff_python_ast`, `ruff_python_trivia`,
`ruff_source_file`, and `ruff_text_size`. The complete upstream notice, including RustPython
attribution, is retained in `vendor/ruff/LICENSE`.

## Vulture

- Project: <https://github.com/jendrikseipp/vulture>
- Revision: `2c21cb0ae2afa657e36f6a397cb573608a65d79e`
- License: MIT
- Local material: `tests/vulture`

The compatibility corpus is adapted from Vulture's tests. Its upstream license is retained beside
the corpus.
