#!/usr/bin/env bash
# Run on the release PR, never in the post-merge publishing workflow.
set -euo pipefail

tag=${1:?Missing version tag}
version=${tag#v}
./scripts/release-metadata.sh check "$tag"
git merge-base --is-ancestor "origin/${DEFAULT_BRANCH:?Missing DEFAULT_BRANCH}" HEAD
git merge-base --is-ancestor "refs/tags/$tag" HEAD
cargo package --locked
cargo publish --locked --registry crates-io --dry-run --no-verify

output=target/release-upload
mkdir -p "$output"
cp "target/package/h4m-$version.crate" "$output/package.crate"
./scripts/release-metadata.sh notes "$tag" > "$output/notes.md"
cargo metadata --locked --offline --format-version 1 | jq --rawfile readme README.md '
    .packages[] | select(.name == "h4m") |
    {
        name, vers: .version,
        deps: [.dependencies[] | {
            name, version_req: .req, features, optional,
            default_features: .uses_default_features, target,
            kind: (.kind // "normal"), registry, explicit_name_in_toml: .rename
        }],
        features, authors, description, documentation, homepage,
        readme: $readme, readme_file: .readme, keywords, categories,
        license, license_file, repository, links, rust_version
    }
' > "$output/metadata.json"

# Cargo's registry upload format: two length-prefixed blobs (JSON, then crate).
# https://doc.rust-lang.org/cargo/reference/registry-web-api.html#publish
length_prefix() {
    local size escaped
    size=$(wc -c < "$1")
    (( size < 4294967296 ))
    printf -v escaped '\\x%02x\\x%02x\\x%02x\\x%02x' \
        "$((size & 255))" "$(((size >> 8) & 255))" \
        "$(((size >> 16) & 255))" "$(((size >> 24) & 255))"
    printf '%b' "$escaped"
}

{
    length_prefix "$output/metadata.json"
    cat "$output/metadata.json"
    length_prefix "$output/package.crate"
    cat "$output/package.crate"
} > "$output/upload.bin"

jq -n --arg tag "$tag" --arg head "$(git rev-parse HEAD)" \
    --arg tree "$(git rev-parse 'HEAD^{tree}')" \
    '{tag: $tag, head: $head, tree: $tree}' > "$output/release.json"
(cd "$output" && sha256sum package.crate metadata.json notes.md upload.bin release.json > SHA256SUMS)
