# Releasing

Push a version tag from the repository's default branch:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The [Prepare release workflow](.github/workflows/prepare-release.yml) creates
`releasing/v0.1.0` and opens a PR against the default branch using `gh api`.
It updates the Cargo version, lockfile, and changelog.

Release notes come from `## Unreleased` in the changelog. If that section is
empty or absent, GitHub generates notes since the previous release. Existing
version entries are preserved. An empty `Unreleased` section is left for future changes.

Review the PR and edit its changelog if needed. The **Release readiness** check
runs Linux/macOS tests, reference equivalence, lints, docs, release automation,
changelog checks, and a Cargo publishing dry-run through Nix. CI saves the
validated crate, notes, and upload payload as an artifact.

The default branch must require **Release readiness** and up-to-date branches.
If `main` advances, update the PR and wait for CI again. After merging, the
[Release workflow](.github/workflows/release.yml):

1. Downloads the artifact for the tested PR commit and matching merged contents.
2. Moves the request tag to the merge commit and uploads the saved payload
   using `CARGO_REGISTRY_TOKEN`.
3. Publishes a GitHub release containing the saved changelog entry.

Publishing uses `nix develop .#release`, containing only upload tools. It sends
the saved bytes to crates.io; no Cargo, builds, or tests run after merging.

All workflows share the Nix cache setup. The only custom secret is
`CARGO_REGISTRY_TOKEN`; GitHub's token handles PRs and releases. Actions must be
allowed to create PRs and update the request tag. Creating the PR explicitly
dispatches CI. Merge through the UI or your own credentials to trigger publishing.

The tag moves to the merge commit. Refresh it locally afterwards with
`git fetch origin tag v0.1.0 --force`.
Tags such as `v0.2.0-rc.1` create GitHub prereleases.

To retry, rerun the failed jobs. Preparation preserves edits to an existing PR.
Publishing skips an existing crate only if its checksum matches the saved
package, then finishes the GitHub release. Published changes need a new version.

If the upload scripts needed a fix, merge it first, then choose **Actions →
Release → Run workflow** on the default branch and enter the original release
PR number. This uses the latest upload scripts with that PR's original validated
artifact and merge commit. No version bump or new tag is needed.

Artifacts are retained for 90 days; refresh an old PR's checks before merging.
Outages, expired credentials, or missing artifacts can still interrupt delivery.
