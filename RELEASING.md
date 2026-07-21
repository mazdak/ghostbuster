# Releasing Ghostbuster

Releases are built from tags on `master`; no release artifact should be built by hand.

1. Update the version in `Cargo.toml` and run `cargo check --locked` to refresh package metadata.
2. Move the relevant notes from `Unreleased` into a dated `CHANGELOG.md` section.
3. Regenerate `THIRD_PARTY.html` with `cargo about generate about.hbs > THIRD_PARTY.html`.
4. Open a pull request and wait for every CI job to pass.
5. Create and push an annotated `vMAJOR.MINOR.PATCH` tag from the merged `master` commit.

The tag workflow verifies the tag/version/changelog match, builds five native archives and five
Python wheels, generates SHA-256 checksums and GitHub provenance attestations, creates the GitHub
release, and publishes the wheels to PyPI.

## One-time PyPI setup

The distribution is named `ghostbuster-code` because `ghostbuster` is already owned by an unrelated
project. Configure a PyPI pending trusted publisher with:

- Owner: `mazdak`
- Repository: `ghostbuster`
- Workflow: `release.yml`
- Environment: `pypi`

Also create a protected GitHub environment named `pypi`. No PyPI API token is required.
