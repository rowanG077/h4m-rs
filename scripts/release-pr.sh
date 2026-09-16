#!/usr/bin/env bash
# Commit prepared metadata and open a release PR using the GitHub REST API.
set -euo pipefail

tag=${1:?Missing version tag}
./scripts/release-metadata.sh notes "$tag" > /dev/null
repository=${GITHUB_REPOSITORY:?Missing GITHUB_REPOSITORY}
base=${DEFAULT_BRANCH:?Missing DEFAULT_BRANCH}
branch="releasing/$tag"
api="repos/$repository"
commit=$(git rev-parse HEAD)
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT

git merge-base --is-ancestor "$commit" "origin/$base"
if [[ -n $(gh api "$api/releases" --paginate --jq ".[] | select(.tag_name == \"$tag\") | .id") ]]; then
    echo "$tag already has a GitHub release" >&2
    exit 1
fi

existing=$(gh api --method GET "$api/pulls" -f state=all \
    -f "head=${repository%%/*}:$branch" -f "base=$base" --jq '.[0].state // empty')
if [[ $existing == closed ]]; then
    echo "The release PR for $tag is already closed; reopen it or use a new version" >&2
    exit 1
fi

head=$(gh api "$api/git/matching-refs/heads/$branch" \
    --jq ".[] | select(.ref == \"refs/heads/$branch\") | .object.sha")
if [[ -z $head ]]; then
    jq -n --arg base "$(git rev-parse 'HEAD^{tree}')" \
        --rawfile manifest Cargo.toml --rawfile lock Cargo.lock --rawfile changelog CHANGELOG.md \
        '{base_tree: $base, tree: [
            {path: "Cargo.toml", mode: "100644", type: "blob", content: $manifest},
            {path: "Cargo.lock", mode: "100644", type: "blob", content: $lock},
            {path: "CHANGELOG.md", mode: "100644", type: "blob", content: $changelog}
        ]}' > "$temporary/tree.json"
    tree=$(gh api --method POST "$api/git/trees" --input "$temporary/tree.json" --jq .sha)
    head=$(gh api --method POST "$api/git/commits" -f "message=Release $tag" \
        -f "tree=$tree" -f "parents[]=$commit" --jq .sha)
    gh api --method POST "$api/git/refs" -f "ref=refs/heads/$branch" -f "sha=$head" --silent
fi

if [[ -z $existing ]]; then
    body="$temporary/body"
    cat > "$body" <<EOF
Prepare $tag: update the Cargo version and lockfile, and finalize the changelog.

Review the release notes in CHANGELOG.md and wait for Release readiness to pass.
CI validates the changelog, runs the full test suite, and prepares the upload.
Merging into $base only uploads the validated crate and creates the GitHub release.
EOF
    gh api --method POST "$api/pulls" -f "title=Release $tag" \
        -f "head=$branch" -f "base=$base" -F "body=@$body" --jq .html_url
fi

# GITHUB_TOKEN-created PRs require approval for automatic PR workflows.
# An explicit dispatch runs CI on the branch immediately, without a PAT.
gh api --method POST "$api/actions/workflows/ci.yml/dispatches" -f "ref=$branch" --silent
