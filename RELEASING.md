# Releasing ai-monitor

`ai-monitor` uses two separate release stages:

1. **release-plz** owns version/changelog preparation and the `v*` Git tag.
2. **cargo-dist** (`.github/workflows/release.yml`) owns cross-platform artifacts, checksums, installers, and the GitHub Release.

This separation is intentional: merging a normal feature PR does **not** publish a release immediately.

## Normal flow

```text
feature/fix PR
    ↓ merge
main
    ↓ Release-plz workflow
release-plz PR
    ├─ Cargo.toml version bump
    ├─ Cargo.lock version bump
    └─ CHANGELOG.md update
    ↓ merge
Release-plz `release`
    ↓
vX.Y.Z tag
    ↓
cargo-dist Release workflow
    ↓
GitHub Release + platform artifacts
    ↓
`ai-monitor update`
```

`release-plz.toml` uses `git_only = true`, so no crates.io token or package publication is required. `git_release_enable = false` keeps GitHub Release creation owned by cargo-dist instead of creating two competing releases.

`release_always = false` is important: only a merged release-plz PR creates a new tag. Ordinary merges merely create or update the pending release PR.

## GitHub repository setting required once

Release-plz uses `GITHUB_TOKEN` to create/update its release PR. In **Settings → Actions → General → Workflow permissions**, enable **Allow GitHub Actions to create and approve pull requests**. The workflow declares only the job-level permissions it needs.

## Versioning policy

While the project is `0.x`, feature commits intentionally advance the minor version (`0.4.x → 0.5.0`). Fix-only releases may remain patch releases when the generated release plan contains no feature-level change.

For the current unreleased Codex multi-bucket and dashboard work, the first release PR after this automation lands is expected to prepare **v0.5.0** from the existing **v0.4.3** tag.

## Why a green `Release` workflow on a PR did not publish

The existing cargo-dist workflow also runs a planning pass for pull requests. PR runs have publishing disabled, so their build/publish jobs are skipped by design. A real GitHub Release is created only after a `v*` tag exists (or a release workflow is manually dispatched for an existing release tag).

## Recovery

If the Release-plz workflow fails to create a PR, first verify the repository setting above. If a `v*` tag exists but the GitHub Release is missing, inspect the cargo-dist `Release` workflow for that tag; do not create a second tag with a different commit just to retry artifact publishing.
