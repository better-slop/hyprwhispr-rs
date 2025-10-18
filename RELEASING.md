# Releasing hyprwhspr-rs

This project ships tagged releases from GitHub Actions. Every artifact assumes the end user installs `whisper.cpp` separately so the local transcription backend is available at runtime.

## Prerequisites

- Install tooling: `cargo install cargo-release git-cliff`.
- Configure GitHub repository secrets:
  - `CRATES_IO_TOKEN` with publish permission (used on stable tags).

## Cutting a prerelease

1. Make sure `CHANGELOG.md` is up to date or run `git-cliff -c git-cliff.toml --tag <next-version>` locally.
2. Run `cargo release --no-dev-version --pre-release alpha --execute` to bump metadata, refresh the changelog, tag `vX.Y.Z-alpha.N`, and push.
3. The `release` workflow builds the Linux binary, publishes a GitHub prerelease with the tarball/checksum, and skips crates.io.

## Cutting a stable release

1. Run `cargo release --execute` to tag `vX.Y.Z` and push.
2. The same workflow republishes the binary and publishes the crate to crates.io because the tag has no prerelease suffix.

## Verifying whisper.cpp availability

The workflow does not bundle `whisper.cpp`; verify installers or downstream packages make it available (`whisper-cpp` package on Arch/AUR, manual build on other distros) before announcing a release.

## Fast VAD feature flag

- Build at least one release candidate with the optional Earshot path so regressions surface early:

  ```bash
  cargo build --release --features fast-vad
  cargo test --features fast-vad
  ```

- Document in the release notes when the flag gains new defaults or behavioral changes so downstream packagers know whether to enable it by default.
