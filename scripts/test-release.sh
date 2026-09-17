#!/usr/bin/env bash
# Exercise release metadata and remote operations in an isolated, mocked repo.
set -euo pipefail

source_root=$(cd "$(dirname "$0")/.." && pwd)
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT
mkdir -p "$temporary/repo/src" "$temporary/repo/scripts" "$temporary/bin"
cp "$source_root"/scripts/{release-metadata,release-pr,package-release,download-release,publish-release}.sh \
    "$temporary/repo/scripts/"
cd "$temporary/repo"
export REAL_CARGO
REAL_CARGO=$(command -v cargo)
export MOCK_LOG="$temporary/operations"
export GITHUB_REPOSITORY=example/h4m DEFAULT_BRANCH=main CARGO_REGISTRY_TOKEN=test-only
export MOCK_RELEASE='' MOCK_HEAD='' MOCK_PR_STATE='' MOCK_DRAFT=true MOCK_STATUS=404
export MOCK_RUN=99 MOCK_SENT="$temporary/sent-upload"
: > "$MOCK_LOG"
: > src/lib.rs
printf '# Test crate\n\nUTF-8: café\n' > README.md
cat > Cargo.toml << 'MANIFEST'
[package]
name = "h4m"
version = "0.1.0"
edition = "2021"
description = "Release test fixture"
license = "MIT"
[package.metadata.test]
version = "must-not-change"
MANIFEST
printf '# Changelog\n\n## 0.1.0\n\n- Initial release.\n' > CHANGELOG.md
cargo generate-lockfile --offline
lock_format=$(sed -n 's/^version = \([0-9]*\)$/\1/p' Cargo.lock)

expect_failure() {
    if "$@" > "$temporary/failure" 2>&1; then
        echo "Expected failure: $*" >&2
        exit 1
    fi
}

./scripts/release-metadata.sh prepare v0.1.0
./scripts/release-metadata.sh check v0.1.0
grep -Fxq '## Unreleased' CHANGELOG.md
grep -Fxq 'version = "must-not-change"' Cargo.toml
expect_failure ./scripts/release-metadata.sh check v0.2.0
expect_failure ./scripts/release-metadata.sh prepare v01.0.0
expect_failure ./scripts/release-metadata.sh prepare 'v0.2.0/unsafe'
cp Cargo.lock "$temporary/lock"
sed 's/0.1.0/0.0.1/' "$temporary/lock" > Cargo.lock
expect_failure ./scripts/release-metadata.sh check v0.1.0
cp "$temporary/lock" Cargo.lock

cat > CHANGELOG.md << 'CHANGELOG'
# Changelog

## Unreleased

- Hand-written notes with "quotes" and `code`.

## 0.1.0

- Initial release.
CHANGELOG
./scripts/release-metadata.sh prepare v0.2.0
./scripts/release-metadata.sh notes v0.2.0 > "$temporary/notes"
grep -q 'Hand-written notes' "$temporary/notes"
expect_failure grep -q 'Initial release' "$temporary/notes"
grep -Fxq "version = $lock_format" Cargo.lock
cp CHANGELOG.md "$temporary/changelog"
./scripts/release-metadata.sh prepare v0.2.0
cmp CHANGELOG.md "$temporary/changelog"
printf '## Changes\n\n- Generated notes.\n' > "$temporary/generated"
./scripts/release-metadata.sh prepare v0.3.0-rc.1 "$temporary/generated"
./scripts/release-metadata.sh notes v0.3.0-rc.1 > "$temporary/notes"
grep -Fxq '### Changes' "$temporary/notes"
grep -Fxq -- '- Generated notes.' "$temporary/notes"
expect_failure ./scripts/release-metadata.sh prepare v0.4.0 /dev/null
cp CHANGELOG.md "$temporary/changelog"
printf '# Changelog\n\n## 0.3.0-rc.1\n' > CHANGELOG.md
expect_failure ./scripts/release-metadata.sh check v0.3.0-rc.1
cp "$temporary/changelog" CHANGELOG.md

# Everything below exercises the real scripts with fake GitHub/Cargo uploads.
cat > "$temporary/bin/gh" << 'MOCK'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOCK_LOG"
if [[ $1 == run && $2 == download ]]; then
    while (($#)); do
        if [[ $1 == --dir ]]; then
            cp "$MOCK_ARTIFACT/"* "$2/"
            break
        fi
        shift
    done
    exit
fi
shift
method=GET
endpoint=
input=
while (($#)); do
    case "$1" in
        --method)
            method=$2
            shift 2
            ;;
        --input)
            input=$2
            shift 2
            ;;
        --jq | -f | -F) shift 2 ;;
        --*) shift ;;
        *)
            endpoint=$1
            shift
            ;;
    esac
done
case "$method ${endpoint#repos/example/h4m/}" in
    'GET actions/workflows/ci.yml/runs') printf '%s' "$MOCK_RUN" ;;
    'GET releases') printf '%s' "$MOCK_RELEASE" ;;
    'GET releases/42') printf '%s' "$MOCK_DRAFT" ;;
    'POST releases') printf '42' ;;
    'GET commits/'*) printf '%s' "$MOCK_TAG_COMMIT" ;;
    'GET pulls') printf '%s' "$MOCK_PR_STATE" ;;
    'GET git/matching-refs/'*) printf '%s' "$MOCK_HEAD" ;;
    'POST git/trees')
        jq -e '.base_tree != null and ([.tree[].path] == ["Cargo.toml", "Cargo.lock", "CHANGELOG.md"])' "$input" > /dev/null
        printf 'tree-sha'
        ;;
    'POST git/commits') printf '%s' "$MOCK_TAG_COMMIT" ;;
    'POST pulls' | 'PATCH releases/42') printf 'https://github.com/example/h4m/example\n' ;;
    'POST git/refs' | 'PATCH git/refs/'* | 'POST actions/workflows/ci.yml/dispatches') ;;
    *)
        echo "Unexpected GitHub API call: $method $endpoint" >&2
        exit 1
        ;;
esac
MOCK
cat > "$temporary/bin/curl" << 'MOCK'
#!/usr/bin/env bash
set -euo pipefail
method=GET
output=
payload=
user_agent=
while (($#)); do
    case "$1" in
        --request)
            method=$2
            shift 2
            ;;
        --output)
            output=$2
            shift 2
            ;;
        --data-binary)
            payload=${2#@}
            shift 2
            ;;
        --user-agent)
            user_agent=$2
            shift 2
            ;;
        *) shift ;;
    esac
done
# crates.io rejects requests without an identifying user agent.
if [[ $user_agent != "h4m-release ($GITHUB_REPOSITORY)" ]]; then
    printf '{"errors":[{"detail":"API data access policy"}]}' > "$output"
    printf '403'
    exit
fi
if [[ $method == PUT ]]; then
    echo 'crate upload' >> "$MOCK_LOG"
    cp "$payload" "$MOCK_SENT"
    if [[ -n ${MOCK_RESPONSE:-} ]]; then
        printf '%s' "$MOCK_RESPONSE" > "$output"
    else
        printf '{"warnings":{}}' > "$output"
    fi
    exit "${MOCK_PUBLISH_FAIL:-0}"
fi
printf '{"version":{"checksum":"%s"}}' "${MOCK_CHECKSUM:-}" > "$output"
printf '%s' "$MOCK_STATUS"
MOCK
cat > "$temporary/bin/cargo" << 'MOCK'
#!/usr/bin/env bash
set -euo pipefail
if [[ ${FORBID_CARGO:-0} == 1 ]]; then
    echo 'Unexpected Cargo call after merge' >> "$MOCK_LOG"
    exit 1
fi
if [[ $1 == publish ]]; then
    [[ " $* " == *' --dry-run '* ]]
    echo 'cargo publish --dry-run' >> "$MOCK_LOG"
    exit 0
fi
exec "$REAL_CARGO" "$@"
MOCK
chmod +x "$temporary/bin/"*
export PATH="$temporary/bin:$PATH"
git init --quiet
# Use command-local identity without touching the developer's Git config.
git add .
git -c user.name=Test -c user.email=test@example.invalid commit --quiet -m Initial
export MOCK_TAG_COMMIT
MOCK_TAG_COMMIT=$(git rev-parse HEAD)
git update-ref refs/remotes/origin/main HEAD
git tag v0.3.0-rc.1

./scripts/release-pr.sh v0.3.0-rc.1 > /dev/null
grep -q 'POST repos/example/h4m/git/refs' "$MOCK_LOG"
grep -q 'POST repos/example/h4m/pulls' "$MOCK_LOG"
grep -q 'POST repos/example/h4m/actions/workflows/ci.yml/dispatches' "$MOCK_LOG"
: > "$MOCK_LOG"
export MOCK_HEAD=$MOCK_TAG_COMMIT MOCK_PR_STATE=open
./scripts/release-pr.sh v0.3.0-rc.1
expect_failure grep -q 'POST repos/example/h4m/git/' "$MOCK_LOG"
expect_failure grep -q 'POST repos/example/h4m/pulls' "$MOCK_LOG"
export MOCK_PR_STATE=closed
expect_failure ./scripts/release-pr.sh v0.3.0-rc.1
export MOCK_PR_STATE=open MOCK_RELEASE=42
expect_failure ./scripts/release-pr.sh v0.3.0-rc.1
export MOCK_RELEASE=''

# Validate and package before merging. Verify both little-endian length prefixes
# and that the upload contains the exact JSON and crate, including UTF-8 text.
mkdir -p target/package
printf 'test archive' > target/package/h4m-0.3.0-rc.1.crate
./scripts/package-release.sh v0.3.0-rc.1
upload=target/release-upload
jq -e '.name == "h4m" and .vers == "0.3.0-rc.1" and .readme_file == "README.md"' "$upload/metadata.json" > /dev/null
read -r -a prefix <<< "$(od -An -tu1 -N4 "$upload/upload.bin")"
json_size=$((prefix[0] + (prefix[1] << 8) + (prefix[2] << 16) + (prefix[3] << 24)))
cmp <(dd if="$upload/upload.bin" bs=1 skip=4 count="$json_size" status=none) "$upload/metadata.json"
read -r -a prefix <<< "$(od -An -tu1 -j "$((4 + json_size))" -N4 "$upload/upload.bin")"
crate_size=$((prefix[0] + (prefix[1] << 8) + (prefix[2] << 16) + (prefix[3] << 24)))
cmp <(dd if="$upload/upload.bin" bs=1 skip="$((8 + json_size))" count="$crate_size" status=none) "$upload/package.crate"
test "$(wc -c < "$upload/upload.bin")" -eq "$((8 + json_size + crate_size))"
export MOCK_ARTIFACT="$temporary/artifact"
cp -r "$upload" "$MOCK_ARTIFACT"

# A base branch advance must require another PR check before merge.
git -c user.name=Test -c user.email=test@example.invalid commit --quiet --allow-empty -m Merge
git update-ref refs/remotes/origin/main HEAD
git checkout --quiet --detach "$MOCK_TAG_COMMIT"
expect_failure ./scripts/package-release.sh v0.3.0-rc.1
git checkout --quiet --detach origin/main

# After merging only artifact lookup and remote operations are allowed.
export FORBID_CARGO=1
rm -r "$upload"
./scripts/download-release.sh v0.3.0-rc.1 "$MOCK_TAG_COMMIT" > /dev/null
expect_failure ./scripts/download-release.sh v0.3.0-rc.1 wrong-head
export MOCK_RUN=''
expect_failure ./scripts/download-release.sh v0.3.0-rc.1 "$MOCK_TAG_COMMIT"
export MOCK_RUN=99
printf 'tampered notes' > "$MOCK_ARTIFACT/notes.md"
expect_failure ./scripts/download-release.sh v0.3.0-rc.1 "$MOCK_TAG_COMMIT"
# Restore exactly the original notes, including their leading blank line.
./scripts/release-metadata.sh notes v0.3.0-rc.1 > "$MOCK_ARTIFACT/notes.md"
./scripts/download-release.sh v0.3.0-rc.1 "$MOCK_TAG_COMMIT" > /dev/null
: > "$MOCK_LOG"
./scripts/publish-release.sh v0.3.0-rc.1 > /dev/null
grep -q 'PATCH repos/example/h4m/git/refs/tags/v0.3.0-rc.1' "$MOCK_LOG"
grep -q 'prerelease=true' "$MOCK_LOG"
grep -q 'crate upload' "$MOCK_LOG"
cmp "$MOCK_SENT" "$MOCK_ARTIFACT/upload.bin"
grep -q 'draft=false' "$MOCK_LOG"

: > "$MOCK_LOG"
export MOCK_STATUS=200 MOCK_RELEASE=42
MOCK_CHECKSUM=$(sha256sum "$upload/package.crate")
export MOCK_CHECKSUM=${MOCK_CHECKSUM%% *}
MOCK_TAG_COMMIT=$(git rev-parse HEAD)
./scripts/publish-release.sh v0.3.0-rc.1 > /dev/null
expect_failure grep -q 'crate upload' "$MOCK_LOG"
expect_failure grep -q 'git/refs/tags' "$MOCK_LOG"
grep -q 'draft=false' "$MOCK_LOG"
: > "$MOCK_LOG"
export MOCK_CHECKSUM=wrong
expect_failure ./scripts/publish-release.sh v0.3.0-rc.1
test ! -s "$MOCK_LOG"
export MOCK_STATUS=503
expect_failure ./scripts/publish-release.sh v0.3.0-rc.1
test ! -s "$MOCK_LOG"
export MOCK_STATUS=404 MOCK_PUBLISH_FAIL=1
expect_failure ./scripts/publish-release.sh v0.3.0-rc.1
expect_failure grep -q 'draft=false' "$MOCK_LOG"
export MOCK_PUBLISH_FAIL=0 MOCK_RESPONSE='{"errors":[{"detail":"registry rejected upload"}]}'
expect_failure ./scripts/publish-release.sh v0.3.0-rc.1
expect_failure grep -q 'draft=false' "$MOCK_LOG"

echo 'Release metadata, PR creation, publishing, and retry checks passed.'
