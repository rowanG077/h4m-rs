#!/usr/bin/env bash
# Upload the exact payload validated on the release PR; no Cargo or test runs.
set -euo pipefail

tag=${1:?Missing version tag}
version=${tag#v}
api="repos/${GITHUB_REPOSITORY:?Missing GITHUB_REPOSITORY}"
commit=$(git rev-parse HEAD)
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT
output=target/release-upload

# On retries, only skip crates.io if the uploaded archive is exactly this build.
status=$(curl --silent --show-error --retry 3 --output "$temporary/registry" \
    --user-agent "h4m-release ($GITHUB_REPOSITORY)" \
    --write-out '%{http_code}' "https://crates.io/api/v1/crates/h4m/$version")
case "$status" in
    200)
        checksum=$(sha256sum "$output/package.crate")
        if [[ ${checksum%% *} != "$(jq -r '.version.checksum' "$temporary/registry")" ]]; then
            echo "crates.io already has different contents for h4m $version" >&2
            exit 1
        fi
        published=true
        ;;
    404)
        : "${CARGO_REGISTRY_TOKEN:?Set the CARGO_REGISTRY_TOKEN Actions secret}"
        published=false
        ;;
    *) echo "crates.io returned HTTP $status; refusing to publish" >&2; exit 1 ;;
esac

# The request tag starts before the metadata commit. Retarget it after merge.
# GITHUB_TOKEN tag updates do not recurse.
tag_commit=$(gh api "$api/commits/$tag" --jq .sha)
if [[ $tag_commit != "$commit" ]]; then
    gh api --method PATCH "$api/git/refs/tags/$tag" \
        -f "sha=$commit" -F force=true --silent
fi

release=$(gh api "$api/releases" --paginate \
    --jq ".[] | select(.tag_name == \"$tag\") | .id")
if [[ -z $release ]]; then
    prerelease=false
    if [[ $version == *-* ]]; then prerelease=true; fi
    release=$(gh api --method POST "$api/releases" -f "tag_name=$tag" \
        -f "name=$tag" -f "target_commitish=$commit" -F "body=@$output/notes.md" \
        -F draft=true -F "prerelease=$prerelease" --jq .id)
fi

if [[ $published == false ]]; then
    curl --fail-with-body --silent --show-error --request PUT \
        --header "Authorization: $CARGO_REGISTRY_TOKEN" \
        --header 'Content-Type: application/octet-stream' \
        --header 'Accept: application/json' \
        --user-agent "h4m-release ($GITHUB_REPOSITORY)" \
        --data-binary "@$output/upload.bin" \
        --output "$temporary/response" https://crates.io/api/v1/crates/new
    if ! jq -e '(.errors // []) | length == 0' "$temporary/response" > /dev/null; then
        jq -r '.errors[]?.detail' "$temporary/response" >&2
        exit 1
    fi
fi

# Leave an already-published release intact, including immutable releases.
if [[ $(gh api "$api/releases/$release" --jq .draft) == true ]]; then
    gh api --method PATCH "$api/releases/$release" \
        -F "body=@$output/notes.md" -F draft=false --jq .html_url
fi
