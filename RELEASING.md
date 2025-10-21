# Releasing hyprwhspr-rs

Releases are automated with [release-plz](https://crates.io/crates/release-plz). Every artifact assumes the end user installs `whisper.cpp` separately so the local transcription backend is available at runtime.

## Prerequisites

- Follow [Conventional Commits](https://www.conventionalcommits.org/):
  - `fix:` → SemVer patch bump.
  - `feat:` → SemVer minor bump.
  - `<prefix>!:` or a `BREAKING CHANGE:` footer → SemVer major bump.
- Keep the repository clean before pushing to `main`; the workflow refuses dirty trees.
- Configure repository secrets:
  - `CRATES_IO_TOKEN` with publish permission (used by the `release-plz release` job for stable releases).

## Automated flow

1. Push changes to `main`.
2. `.github/workflows/release-plz.yml` runs `release-plz release-pr`, updating or opening a `release-plz-*` pull request with the proposed version bump and `CHANGELOG.md` entry.
3. Review the PR:
   - Validate the suggested SemVer bump.
   - Tidy the generated changelog if needed.
   - Ensure preflight checks (`cargo fmt`, `cargo clippy --all-targets`, `cargo test`, `cargo build --release`) pass on your branch.
4. Merge the release PR once it looks right.
5. The same workflow runs `release-plz release`, which:
   - Publishes updated crates to crates.io.
   - Tags the commit (`vX.Y.Z`).
6. The tag triggers `.github/workflows/release.yml`, which builds the optimized Linux binary, bundles the tarball plus checksum, and publishes the GitHub release using the `CHANGELOG.md` entry for the tag.

> Trusted publishing is supported by deleting `CARGO_REGISTRY_TOKEN` and granting `id-token: write` to the `release` job if you prefer that model.

## Manual overrides

- To preview the upcoming release locally, run `release-plz release-pr --dry-run`.
- To force a publish from your machine (rarely necessary), run `release-plz release --dry-run` first, then `release-plz release` with `CARGO_REGISTRY_TOKEN` exported. Push the tags it produces so CI can finish the artifact build.

## Pre-release builds

Release-plz focuses on SemVer stable tags. If you need alpha/beta channels, create a dedicated branch and configure release-plz accordingly (see the [release-plz docs](https://release-plz.dev/docs/)). Coordinate with the team before diverging from the automated stable flow.
