#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECKER="${SCRIPT_DIR}/check-migration-fixture-regeneration-record.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$TMP/.github/scripts" "$TMP/src-tauri/tests/fixtures/migrations"
cp "$CHECKER" "$TMP/.github/scripts/"

git -C "$TMP" init --quiet
git -C "$TMP" config user.name "GMM CI self-test"
git -C "$TMP" config user.email "ci-self-test@example.invalid"

printf '%s\n' "old-checksum  fixture.db" >"$TMP/src-tauri/tests/fixtures/migrations/SHA256SUMS"
printf '%s\n' "# Migration fixture regenerations" >"$TMP/src-tauri/tests/fixtures/migrations/REGENERATIONS.md"
git -C "$TMP" add .
git -C "$TMP" commit --quiet -m "baseline"
BASE_COMMIT="$(git -C "$TMP" rev-parse HEAD)"

printf '%s\n' "new-checksum  fixture.db" >"$TMP/src-tauri/tests/fixtures/migrations/SHA256SUMS"
git -C "$TMP" add src-tauri/tests/fixtures/migrations/SHA256SUMS
git -C "$TMP" commit --quiet -m "change checksum only"
CHECKSUM_ONLY_COMMIT="$(git -C "$TMP" rev-parse HEAD)"

if output="$(cd "$TMP" && .github/scripts/check-migration-fixture-regeneration-record.sh \
  "$BASE_COMMIT" "$CHECKSUM_ONLY_COMMIT" 2>&1)"; then
  echo "regeneration-record guard accepted a checksum change without a record" >&2
  exit 1
fi
case "$output" in
  *"REGENERATIONS.md did not"*"record which fixture changed and why"*) ;;
  *)
    echo "guard failed without explaining the missing regeneration record: $output" >&2
    exit 1
    ;;
esac
echo "checksum-only mutation was rejected with the required explanation"

printf '%s\n' "- fixture.db: legitimate self-test rewrite" >>"$TMP/src-tauri/tests/fixtures/migrations/REGENERATIONS.md"
git -C "$TMP" add src-tauri/tests/fixtures/migrations/REGENERATIONS.md
git -C "$TMP" commit --quiet -m "record regeneration reason"
RECORDED_COMMIT="$(git -C "$TMP" rev-parse HEAD)"

output="$(cd "$TMP" && .github/scripts/check-migration-fixture-regeneration-record.sh \
  "$BASE_COMMIT" "$RECORDED_COMMIT" 2>&1)"
case "$output" in
  *"checksums and their regeneration record both changed"*) ;;
  *)
    echo "guard passed without confirming the checksum and record changes: $output" >&2
    exit 1
    ;;
esac
echo "checksum-plus-record mutation was accepted"
