#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${1:-$SCRIPT_DIR}"

mapfile -t scripts < <(find "$ROOT" -type f -name '*.sh' -print | LC_ALL=C sort)
[ "${#scripts[@]}" -gt 0 ] || { echo "no shell scripts found" >&2; exit 1; }

for script in "${scripts[@]}"; do
  bash -n "$script"
done
