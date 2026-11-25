# Release integrity and build provenance

The Rust installer is experimental. These checks improve traceability; they are not a claim that the installer is complete or that every release is independently reproducible.

## Distribution channels

- **Prebuilt executable:** download the x86_64 GNU or musl archive from the project's [GitHub Releases](https://github.com/decksters-lab/artix-install/releases). The musl archive is intended for Artix live environments without a Rust toolchain.
- **Build from source:** inspect the source in this repository and build it yourself with Rust 1.88 or newer:

  ```sh
  cd rust
  cargo build --release
  ```

  The resulting executable is `target/release/artix-setup`. Source builds are available now, but dependency pinning is not yet at the standard required for reproducible release builds: a committed `Cargo.lock` and locked CI builds are still required.
- **Package-manager recipe:** an official PKGBUILD / Artix package is not published yet. It should be added only after the source and dependency lock are pinned and the package can be built and tested in a clean Artix environment.

## Preview release publication

The feature-branch workflow publishes a preview release only when a maintainer explicitly marks a commit message with `[publish-preview]`. The workflow builds and smoke-tests both x86_64 archives first, then creates the `v0.1.0-test3` preview tag at that exact commit. Ordinary branch pushes and pull-request builds do not overwrite a published release. This keeps the release tag, source commit, and packaged binaries aligned.

## Verify an archive checksum

Download the archive and its matching `.sha256` file from the same GitHub Release, then run:

```sh
sha256sum -c artix-setup-x86_64-linux-musl.tar.gz.sha256
```

Use the GNU filename instead if you downloaded the GNU archive.

A matching checksum detects accidental corruption or a mismatch between the archive and checksum file. **A checksum downloaded from the same release is not, by itself, proof of publisher authenticity**: someone able to replace both files could also replace the checksum.

## Verify the GitHub build attestation

For release/tag builds, GitHub Actions creates a signed build-provenance attestation for each packaged archive. Once an attestation is published, install the GitHub CLI and run:

```sh
gh attestation verify artix-setup-x86_64-linux-musl.tar.gz \
  --repo decksters-lab/artix-install \
  --signer-workflow .github/workflows/standalone-binaries.yml
```

This checks the artifact's provenance against GitHub's attestation service and restricts the expected signer workflow. Use the GNU archive filename for the GNU build. Pull-request builds intentionally do not publish attestations.

Attestations establish which repository workflow produced an artifact; they do not prove that the source code is safe, that the build is reproducible, or that the installer is free of defects. Review the source, workflow, commit, and release notes as well.

## What is still needed before a production release

1. Commit `rust/Cargo.lock` and use `cargo build --locked` in CI and documented source builds.
2. Pin third-party GitHub Actions to reviewed commit SHAs.
3. Add an independently reproducible build process and publish comparison results.
4. Publish an Artix PKGBUILD/package built from a pinned source commit and lockfile.
5. Expand disposable-VM testing to cover the full install and recovery paths.
6. Keep the safety model and implemented-feature list synchronized with the executable.

Until those items and full VM tests are complete, use the existing Bash installer for real installations. The Rust prototype's disk-writing and base-install stages are not a complete operating-system installation.
