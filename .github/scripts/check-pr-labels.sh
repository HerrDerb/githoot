#!/usr/bin/env bash
# Checks a pull request's labels for the release notes: exactly one category label, plus `breaking`
# optionally.
#
# Usage: check-pr-labels.sh '<labels as a JSON array of names>'
# The sections these labels become are in .github/release.yml.
set -euo pipefail

categories=(feature improvement bug security dependencies internal)
labels="$1"

has() { jq -e --arg l "$1" 'index($l) != null' <<<"$labels" >/dev/null; }

found=()
for c in "${categories[@]}"; do
  if has "$c"; then found+=("$c"); fi
done

if [ "${#found[@]}" -ne 1 ]; then
  echo "::error::A pull request needs exactly one of: ${categories[*]}. It has ${#found[@]}${found:+ (${found[*]})}. Add \`breaking\` on top when it changes something people rely on."
  exit 1
fi

echo "Release notes section: ${found[0]}$(has breaking && echo ', listed under Breaking changes' || true)"
