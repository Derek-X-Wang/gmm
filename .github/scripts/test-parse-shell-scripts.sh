#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PARSER="${SCRIPT_DIR}/parse-shell-scripts.sh"
FIXTURES="${SCRIPT_DIR}/../test-fixtures/shell-parse"

if output="$($PARSER "$FIXTURES" 2>&1)"; then
  echo "parse-shell-scripts.sh accepted a syntax error in a non-first file" >&2
  exit 1
fi

case "$output" in
  *02-invalid.sh*) ;;
  *)
    echo "parser failed without naming the non-first invalid fixture: $output" >&2
    exit 1
    ;;
esac

echo "shell parser self-test rejected the non-first invalid fixture"
