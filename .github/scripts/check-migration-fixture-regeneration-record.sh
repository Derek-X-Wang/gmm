#!/usr/bin/env bash

set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 BASE_COMMIT HEAD_COMMIT" >&2
  exit 2
fi

BASE_COMMIT="$1"
HEAD_COMMIT="$2"
CHECKSUMS="src-tauri/tests/fixtures/migrations/SHA256SUMS"
REGENERATIONS="src-tauri/tests/fixtures/migrations/REGENERATIONS.md"

git rev-parse --verify --quiet "${BASE_COMMIT}^{commit}" >/dev/null || {
  echo "could not resolve pull request base commit: ${BASE_COMMIT}" >&2
  exit 2
}
git rev-parse --verify --quiet "${HEAD_COMMIT}^{commit}" >/dev/null || {
  echo "could not resolve pull request head commit: ${HEAD_COMMIT}" >&2
  exit 2
}

if git diff --quiet "$BASE_COMMIT" "$HEAD_COMMIT" -- "$CHECKSUMS"; then
  echo "Migration fixture checksums are unchanged; no regeneration record is required."
  exit 0
fi

if git diff --quiet "$BASE_COMMIT" "$HEAD_COMMIT" -- "$REGENERATIONS"; then
  echo "::error file=${CHECKSUMS}::${CHECKSUMS} changed, but ${REGENERATIONS} did not. Historical migration fixtures are immutable evidence; record which fixture changed and why the rewrite was legitimate."
  echo "This gate makes the reason mandatory on the ordinary pull-request path; it is not a tamper-proof check."
  exit 1
fi

echo "Migration fixture checksums and their regeneration record both changed."
