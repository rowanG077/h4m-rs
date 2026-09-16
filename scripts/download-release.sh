#!/usr/bin/env bash
# Fetch the artifact approved by CI for exactly this PR head, without rebuilding.
set -euo pipefail

tag=${1:?Missing version tag}
head=${2:?Missing release PR head SHA}
repository=${GITHUB_REPOSITORY:?Missing GITHUB_REPOSITORY}
api="repos/$repository"
output=target/release-upload

run=$(gh api --method GET "$api/actions/workflows/ci.yml/runs" \
    -f "head_sha=$head" -f "branch=releasing/$tag" -f status=success --paginate \
    --jq ".workflow_runs[] | select(.head_repository.full_name == \"$repository\") | .id" | sed -n '1p')
if [[ -z $run ]]; then
    echo "No successful CI run for release PR commit $head" >&2
    exit 1
fi
mkdir -p "$output"
gh run download "$run" --repo "$repository" --name "release-$head" --dir "$output"

# Require the tested PR contents, including when squash-merging or rebasing.
jq -e --arg tag "$tag" --arg head "$head" --arg tree "$(git rev-parse 'HEAD^{tree}')" \
    '.tag == $tag and .head == $head and .tree == $tree' "$output/release.json" > /dev/null
(cd "$output" && sha256sum --check SHA256SUMS)
