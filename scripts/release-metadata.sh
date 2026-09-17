#!/usr/bin/env bash
set -euo pipefail

command=${1:?Usage: release-metadata.sh prepare|check|notes TAG [FALLBACK_NOTES]}
tag=${2:?Missing version tag}
if [[ ! $tag =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?$ ]]; then
    echo "Expected a version tag such as v0.1.0 or v0.2.0-rc.1" >&2
    exit 1
fi
version=${tag#v}

notes() {
    awk -v heading="## $1" '
        /^## / { active = ($0 == heading); next }
        active { print }
    ' CHANGELOG.md
}

case "$command" in
    prepare)
        temporary=$(mktemp -d)
        trap 'rm -rf "$temporary"' EXIT

        if grep -Fxq "## $version" CHANGELOG.md; then
            cp CHANGELOG.md "$temporary/changelog"
        else
            notes Unreleased > "$temporary/notes"
            if ! grep -q '[^[:space:]]' "$temporary/notes"; then
                sed 's/^## /### /' "${3:?No Unreleased notes; supply generated release notes}" > "$temporary/notes"
            fi
            if ! grep -q '[^[:space:]]' "$temporary/notes"; then
                echo "Release notes must not be empty" >&2
                exit 1
            fi
            {
                printf '# Changelog\n\n## Unreleased\n\n## %s\n\n' "$version"
                cat "$temporary/notes"
                printf '\n'
                awk '
                    /^## / { active = ($0 != "## Unreleased") }
                    active { print }
                ' CHANGELOG.md
            } > "$temporary/changelog"
        fi
        if ! grep -Fxq '## Unreleased' "$temporary/changelog"; then
            awk 'NR == 1 { print; print "\n## Unreleased"; next } { print }' \
                "$temporary/changelog" > "$temporary/with-unreleased"
            mv "$temporary/with-unreleased" "$temporary/changelog"
        fi

        # Only replace package.version, leaving dependency versions alone.
        awk -v version="$version" '
            /^\[/ { package = ($0 == "[package]") }
            package && /^version[[:space:]]*=/ {
                $0 = "version = \"" version "\""
                changed++
            }
            { print }
            END { if (changed != 1) exit 1 }
        ' Cargo.toml > "$temporary/manifest"
        cp "$temporary/manifest" Cargo.toml
        # Cargo updates the root entry without changing pinned dependencies.
        cargo metadata --offline --format-version 1 > /dev/null
        cp "$temporary/changelog" CHANGELOG.md
        "$0" check "$tag"
        ;;
    check)
        actual=$(cargo metadata --locked --offline --format-version 1 |
            jq -r '.packages[] | select(.name == "h4m") | .version')
        if [[ $actual != "$version" ]]; then
            echo "Tag $tag does not match Cargo.toml version $actual" >&2
            exit 1
        fi
        if [[ $(grep -Fxc "## $version" CHANGELOG.md) != 1 ]] ||
            ! notes "$version" | grep -q '[^[:space:]]'; then
            echo "CHANGELOG.md needs one nonempty '## $version' entry" >&2
            exit 1
        fi
        ;;
    notes)
        notes "$version"
        ;;
    *)
        echo "Unknown release metadata command: $command" >&2
        exit 1
        ;;
esac
