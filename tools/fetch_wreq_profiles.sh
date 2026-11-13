#!/usr/bin/env bash
# Download the wreq-util browser profiles into tools/profile/ (input for gen.py).
#
#   tools/fetch_wreq_profiles.sh            # pinned revision below
#   tools/fetch_wreq_profiles.sh <rev>      # any commit / tag / branch
#
# Result: tools/profile/{chrome,firefox,safari,opera,okhttp}.rs + subfolders,
# i.e. a copy of wreq-util's src/emulate/profile. The folder is git-ignored.
set -euo pipefail

REPO="https://github.com/0x676e67/wreq-util"
# Revision the committed conf/fingerprint/** was generated from.
REV_DEFAULT="917d2581d6761a94db66466caa6c2eb4d4cdbd43"
REV="${1:-$REV_DEFAULT}"

cd "$(dirname "${BASH_SOURCE[0]}")"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

git init -q "$tmp"
git -C "$tmp" remote add origin "$REPO"
git -C "$tmp" fetch -q --depth 1 origin "$REV"
git -C "$tmp" checkout -q FETCH_HEAD

src="$tmp/src/emulate/profile"
[ -d "$src" ] || { echo "error: src/emulate/profile not found at $REV" >&2; exit 1; }

rm -rf profile
cp -r "$src" profile
echo "wreq-util @ $(git -C "$tmp" rev-parse HEAD) -> tools/profile/"
