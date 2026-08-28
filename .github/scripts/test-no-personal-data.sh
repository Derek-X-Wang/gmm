#!/usr/bin/env bash

# A guard nobody has watched fail is not a guard. Each case below builds a
# throwaway repository containing exactly one condition and asserts the
# checker's verdict on it.
#
# Fixtures deliberately live at varied, nested paths that resemble real project
# files. An earlier version put every fixture in one file at the repository
# root, which let a mutation narrowing the checker's pathspec to that single
# file keep every case green while production scanning was gutted. A suite that
# cannot detect its own defeat is not evidence.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECKER="${SCRIPT_DIR}/check-no-personal-data.sh"
failures=0

# Deliberately not a real contributor's account name: this file is public too.
PERSONAL_ACCOUNT="jrivera"

# ANSI-C quoting yields one literal backslash without placing it against a
# closing quote, which shellcheck would otherwise read as a botched escape.
BS=$'\\'

setup_repo() {
  local dir="$1"
  mkdir -p "$dir"
  git -C "$dir" init --quiet
  git -C "$dir" config user.email "guard-self-test@example.invalid"
  git -C "$dir" config user.name "Guard Self Test"
  mkdir -p "$dir/src" "$dir/docs" "$dir/src-tauri/src/core"
  printf '%s\n' "baseline" >"$dir/src/main.rs"
  git -C "$dir" add src/main.rs
  git -C "$dir" commit --quiet -m "baseline"
}

expect() {
  local name="$1" want="$2" base="$3" head="$4" dir="$5"
  local output status
  set +e
  output="$(cd "$dir" && "$CHECKER" "$base" "$head" 2>&1)"
  status=$?
  set -e
  if [ "$status" -ne "$want" ]; then
    echo "FAIL: ${name}: expected exit ${want}, got ${status}"
    printf '%s\n' "$output" | sed 's/^/    /'
    failures=$((failures + 1))
  else
    echo "ok: ${name}"
  fi
}

# Each case writes its fixture to its own path, so no single pathspec covers
# the suite.
run_case() {
  local name="$1" want="$2" message="$3" fixture_path="$4" added_line="$5"
  local dir
  dir="$(mktemp -d)"
  trap 'rm -rf "$dir"' RETURN
  setup_repo "$dir"
  local base
  base="$(git -C "$dir" rev-parse HEAD)"
  mkdir -p "$dir/$(dirname "$fixture_path")"
  printf '%s\n' "$added_line" >>"$dir/$fixture_path"
  git -C "$dir" add "$fixture_path"
  git -C "$dir" commit --quiet -m "$message"
  expect "$name" "$want" "$base" "$(git -C "$dir" rev-parse HEAD)" "$dir"
}

run_case "a clean commit passes" 0 \
  "Fix the thing" "src/main.rs" "a harmless line"

run_case "a session trailer in the message fails" 1 \
  "Fix the thing

Claude-Session: https://claude.ai/code/session_0123456789ABCDEF" \
  "src/lib.rs" "a harmless line"

run_case "a session link in an added line fails" 1 \
  "Fix the thing" "docs/notes.md" \
  "see https://claude.ai/code/session_0123456789ABCDEF for context"

run_case "a personal home path in the message fails" 1 \
  "Fix the thing

Per /Users/${PERSONAL_ACCOUNT}/notes.md this was intentional." \
  "src-tauri/src/core/mod.rs" "a harmless line"

run_case "a personal home path in an added line fails" 1 \
  "Fix the thing" "src-tauri/src/core/importer.rs" \
  "const p = '/home/${PERSONAL_ACCOUNT}/config.toml';"

WIN_ESCAPED="let p = \"C:${BS}${BS}Users${BS}${BS}${PERSONAL_ACCOUNT}${BS}${BS}AppData\";"
WIN_RAW="installed under C:${BS}Users${BS}${PERSONAL_ACCOUNT}${BS}Games"
WIN_LOWER="probe c:${BS}users${BS}${PERSONAL_ACCOUNT}${BS}Saved"
JSON_ESCAPED="{\"path\": \"${BS}/home${BS}/${PERSONAL_ACCOUNT}${BS}/data\"}"

run_case "a source-escaped Windows home path fails" 1 \
  "Fix the thing" "src-tauri/src/core/junction.rs" "$WIN_ESCAPED"

run_case "a raw Windows home path fails" 1 \
  "Fix the thing" "docs/install.md" "$WIN_RAW"

run_case "a lowercase Windows home path fails" 1 \
  "Fix the thing" "docs/troubleshooting.md" "$WIN_LOWER"

run_case "a JSON-escaped home path fails" 1 \
  "Fix the thing" "src/fixtures/session.json" "$JSON_ESCAPED"

# A diff line for added source beginning with '++' is spelled '+++...', which a
# header filter written as '^+++' silently discards along with the leak.
run_case "a leak on a line starting with ++ fails" 1 \
  "Fix the thing" "src/counter.ts" \
  "++counter; // /home/${PERSONAL_ACCOUNT}/private"

run_case "a CI runner path passes" 0 \
  "Fix the thing" "docs/ci.md" "workdir: /home/runner/work/gmm"

run_case "a Windows CI runner path passes" 0 \
  "Fix the thing" "docs/ci.md" \
  "workdir: C:${BS}Users${BS}runneradmin${BS}work"

run_case "a placeholder account passes" 0 \
  "Fix the thing" "docs/example.md" "example: /Users/username/Games"

# The guard exempts its own two files because they must carry specimens. That
# exemption has to be exactly two paths wide: a leak in any other file, in the
# same pull request that edits them, must still fail.
scoped_dir="$(mktemp -d)"
trap 'rm -rf "$scoped_dir"' EXIT
setup_repo "$scoped_dir"
scoped_base="$(git -C "$scoped_dir" rev-parse HEAD)"
mkdir -p "$scoped_dir/.github/scripts"
printf '%s\n' "Claude-Session: https://claude.ai/code/session_SPECIMEN" \
  >"$scoped_dir/.github/scripts/test-no-personal-data.sh"
git -C "$scoped_dir" add .github/scripts/test-no-personal-data.sh
git -C "$scoped_dir" commit --quiet -m "edit the guard self-test"
expect "a specimen inside the guard self-test passes" 0 \
  "$scoped_base" "$(git -C "$scoped_dir" rev-parse HEAD)" "$scoped_dir"

printf '%s\n' "leak: /home/${PERSONAL_ACCOUNT}/notes" \
  >>"$scoped_dir/src-tauri/src/core/variants.rs"
git -C "$scoped_dir" add src-tauri/src/core/variants.rs
git -C "$scoped_dir" commit --quiet -m "and a leak elsewhere"
expect "a leak outside the guard is still caught" 1 \
  "$scoped_base" "$(git -C "$scoped_dir" rev-parse HEAD)" "$scoped_dir"

# Removing an existing leak must not be blocked by the leak it removes.
removal_dir="$(mktemp -d)"
setup_repo "$removal_dir"
printf '%s\n' "old: /home/${PERSONAL_ACCOUNT}/thing" >>"$removal_dir/src/main.rs"
git -C "$removal_dir" add src/main.rs
git -C "$removal_dir" commit --quiet -m "baseline with a leak"
removal_base="$(git -C "$removal_dir" rev-parse HEAD)"
printf '%s\n' "baseline" >"$removal_dir/src/main.rs"
git -C "$removal_dir" add src/main.rs
git -C "$removal_dir" commit --quiet -m "Remove the path"
expect "deleting an existing leak passes" 0 \
  "$removal_base" "$(git -C "$removal_dir" rev-parse HEAD)" "$removal_dir"
rm -rf "$removal_dir"

# An unresolvable endpoint must abort, never read as "nothing found".
endpoint_dir="$(mktemp -d)"
setup_repo "$endpoint_dir"
expect "an unresolvable base commit exits 2" 2 \
  "0000000000000000000000000000000000000000" \
  "$(git -C "$endpoint_dir" rev-parse HEAD)" "$endpoint_dir"
rm -rf "$endpoint_dir"

# grep exits 1 for "no match" and 2 or more for a real failure. Collapsing
# those is the same mistake the gate exists to prevent, so a broken grep must
# abort rather than report a clean tree. A shim earlier on PATH forces it.
broken_dir="$(mktemp -d)"
setup_repo "$broken_dir"
broken_base="$(git -C "$broken_dir" rev-parse HEAD)"
printf '%s\n' "a harmless line" >>"$broken_dir/src/main.rs"
git -C "$broken_dir" add src/main.rs
git -C "$broken_dir" commit --quiet -m "Fix the thing"
mkdir -p "$broken_dir/bin"
printf '%s\n' '#!/bin/sh' 'exit 2' >"$broken_dir/bin/grep"
chmod +x "$broken_dir/bin/grep"
broken_output=""
broken_status=0
set +e
broken_output="$(cd "$broken_dir" && PATH="$broken_dir/bin:$PATH" \
  "$CHECKER" "$broken_base" "$(git -C "$broken_dir" rev-parse HEAD)" 2>&1)"
broken_status=$?
set -e
if [ "$broken_status" -ne 2 ]; then
  echo "FAIL: a failing grep aborts: expected exit 2, got ${broken_status}"
  printf '%s\n' "$broken_output" | sed 's/^/    /'
  failures=$((failures + 1))
else
  echo "ok: a failing grep aborts"
fi
rm -rf "$broken_dir"

if [ "$failures" -gt 0 ]; then
  echo "${failures} self-test case(s) failed"
  exit 1
fi

echo "personal-data guard self-tests passed"
