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
    ↓ explicit workflow_dispatch bridge
cargo-dist Release workflow
    ↓
GitHub Release + platform artifacts
    ↓
`ai-monitor update`
```

`release-plz.toml` uses `git_only = true` and `publish = false`, so no crates.io token or package publication is required. `git_release_enable = false` keeps GitHub Release creation owned by cargo-dist instead of creating two competing releases.

`release_always = false` is important: only a merged release-plz PR creates a new tag. Ordinary merges merely create or update the pending release PR.

## Why the dispatch bridge exists

Release-plz creates tags using the repository `GITHUB_TOKEN`. GitHub intentionally prevents most events created by `GITHUB_TOKEN` from starting another workflow, so the resulting tag push does **not** reliably trigger the cargo-dist workflow's `push.tags` listener.

The Release-plz workflow therefore checks whether the current `vX.Y.Z` tag exists without a matching GitHub Release and explicitly starts `.github/workflows/release.yml` with `workflow_dispatch`. `workflow_dispatch` is one of GitHub's supported exceptions to the recursion-prevention rule.

The dispatch is idempotent: if the tag does not exist yet, or the GitHub Release already exists, it does nothing.

## GitHub repository setting required once

Release-plz uses `GITHUB_TOKEN` to create/update its release PR. In **Settings → Actions → General → Workflow permissions**, enable **Allow GitHub Actions to create and approve pull requests**. The workflow declares only the job-level permissions it needs.

## Versioning policy

While the project is `0.x`, feature commits intentionally advance the minor version (`0.4.x → 0.5.0`). Fix-only releases may remain patch releases when the generated release plan contains no feature-level change.

## Why a green `Release` workflow on a PR did not publish

The cargo-dist workflow also runs a planning pass for pull requests. PR runs have publishing disabled, so their build/publish jobs are skipped by design. A real GitHub Release is created only for an existing `v*` tag.

## Manual recovery

If a `v*` tag exists but the GitHub Release is missing, manually dispatch the **Release** workflow with the full tag, including the leading `v` (for example `v0.5.0`). Entering `0.5.0` will fail checkout because that ref does not exist.

Do not create a second tag with a different commit just to retry artifact publishing.
