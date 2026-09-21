#!/usr/bin/env bash
set -euo pipefail

: "${VERSION:?expected release version}"
: "${GITHUB_SHA:?expected tested commit}"
: "${GITHUB_REPOSITORY:?expected repository}"
version=$(python3 scripts/version.py)
test "$version" = "$VERSION"
tag="v$version"
# Do not move a tag, including when a previous attempt stopped after tagging.
git fetch origin --tags
if git show-ref --verify --quiet "refs/tags/$tag"; then
  test "$(git rev-parse "$tag^{commit}")" = "$GITHUB_SHA" || {
    echo "$tag points at another commit; refusing to publish" >&2
    exit 1
  }
else
  git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
  git config user.name 'github-actions[bot]'
  git tag -a "$tag" "$GITHUB_SHA" -m "Release $tag"
  git push origin "refs/tags/$tag"
fi

# A failed API request must not be mistaken for a missing release.
state=$(gh api --paginate "repos/$GITHUB_REPOSITORY/releases" \
  --jq ".[] | select(.tag_name == \"$tag\") | .draft")
if [ "$state" = false ]; then
  echo "$tag is already published; leaving its assets unchanged"
  exit 0
fi

notes=$(mktemp)
trap 'rm -f "$notes"' EXIT
cat > "$notes" <<EOF
CLI and GUI packages for x86_64-linux are available from the kabilan108 Cachix cache.

With Nix, flakes, and the Cachix client installed:

\`\`\`sh
cachix use kabilan108
nix profile install github:kabilan108/dictator/$tag#default github:kabilan108/dictator/$tag#gui
dictator version
\`\`\`

On NixOS, configure the cache declaratively as described in the README.

The CLI tarball supports x86_64 Linux with glibc 2.35 or newer. It does not
require Nix. Install the audio and desktop tools listed in its README.
The GUI is distributed through Nix. SHA256SUMS covers the CLI archive.
EOF
if [ -z "$state" ]; then
  gh release create "$tag" --verify-tag --draft --title "Dictator $tag" --notes-file "$notes"
fi
gh release upload "$tag" release-assets/*.tar.gz release-assets/SHA256SUMS --clobber
gh release edit "$tag" --draft=false --notes-file "$notes"
