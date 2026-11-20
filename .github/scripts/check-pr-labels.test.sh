#!/usr/bin/env bash
# Tests for check-pr-labels.sh. Run: bash .github/scripts/check-pr-labels.test.sh
set -u
here="$(cd "$(dirname "$0")" && pwd)"
failed=0

expect() { # expect <pass|fail> <labels json> <why>
  if bash "$here/check-pr-labels.sh" "$2" >/dev/null 2>&1; then got=pass; else got=fail; fi
  if [ "$got" = "$1" ]; then echo "ok   $3"; else echo "FAIL $3: expected $1, got $got for $2"; failed=1; fi
}

expect pass '["feature"]'                         "one category"
expect pass '["bug","breaking"]'                  "a category plus breaking"
expect pass '["internal","good first issue"]'     "labels outside the set are ignored"
for c in feature improvement bug security dependencies internal; do
  expect pass "[\"$c\"]"                          "$c alone"
done
expect fail '[]'                                  "no label"
expect fail '["breaking"]'                        "breaking alone is not a category"
expect fail '["feature","bug"]'                   "two categories"
expect fail '["internal","breaking"]'             "breaking cannot hide under internal"
expect fail '["Feature"]'                         "names are exact"

exit $failed
