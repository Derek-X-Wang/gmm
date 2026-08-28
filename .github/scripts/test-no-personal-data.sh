#!/usr/bin/env bash

# A guard nobody has watched fail is not a guard. Each case below builds a
# throwaway repository containing exactly one condition and asserts the
# checker's verdict on it, so a change that makes the checker silently
# permissive fails here.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECKER="${SCRIPT_DIR}/check-no-personal-data.sh"
failures=0

# Deliberately not a real contributor's account name: this file is public too.
PERSONAL_ACCOUNT="jrivera"

setup_repo() {
  local dir="$1"
  mkdir -p "$dir"
  git -C "$dir" init --quiet
  git -C "$dir" config user.email "guard-self-test@example.invalid"
  git -C "$dir" config user.name "Guard Self Test"
  printf '%s\n' "baseline" >"$dir/file.txt"
  git -C "$dir" add file.txt
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

run_case() {
  local name="$1" want="$2" message="$3" added_line="$4"
  local dir
  dir="$(mktemp -d)"
  trap 'rm -rf "$dir"' RETURN
  setup_repo "$dir"
  local base
  base="$(git -C "$dir" rev-parse HEAD)"
  printf '%s\n' "$added_line" >>"$dir/file.txt"
  git -C "$dir" add file.txt
  git -C "$dir" commit --quiet -m "$message"
  expect "$name" "$want" "$base" "$(git -C "$dir" rev-parse HEAD)" "$dir"
}

run_case "a clean commit passes" 0 \
  "Fix the thing" "a harmless line"

run_case "a session trailer in the message fails" 1 \
  "Fix the thing

Claude-Session: https://claude.ai/code/session_0123456789ABCDEF" \
  "a harmless line"

run_case "a session link in an added line fails" 1 \
  "Fix the thing" \
  "see https://claude.ai/code/session_0123456789ABCDEF for context"

run_case "a personal home path in the message fails" 1 \
  "Fix the thing

Per /Users/${PERSONAL_ACCOUNT}/notes.md this was intentional." \
  "a harmless line"

run_case "a personal home path in an added line fails" 1 \
  "Fix the thing" \
  "const p = '/home/${PERSONAL_ACCOUNT}/config.toml';"

# ANSI-C quoting yields one literal backslash without placing it against a
# closing quote, which shellcheck would otherwise read as a botched escape.
BS=$'\\'
WIN_ESCAPED="let p = \"C:${BS}${BS}Users${BS}${BS}${PERSONAL_ACCOUNT}${BS}${BS}AppData\";"
WIN_RAW="installed under C:${BS}Users${BS}${PERSONAL_ACCOUNT}${BS}Games"

run_case "a source-escaped Windows home path fails" 1 \
  "Fix the thing" "$WIN_ESCAPED"

run_case "a raw Windows home path fails" 1 \
  "Fix the thing" "$WIN_RAW"

run_case "a CI runner path passes" 0 \
  "Fix the thing" "workdir: /home/runner/work/gmm"

run_case "a placeholder account passes" 0 \
  "Fix the thing" "example: /Users/username/Games"

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

printf '%s\n' "leak: /home/${PERSONAL_ACCOUNT}/notes" >>"$scoped_dir/file.txt"
git -C "$scoped_dir" add file.txt
git -C "$scoped_dir" commit --quiet -m "and a leak elsewhere"
expect "a leak outside the guard is still caught" 1 \
  "$scoped_base" "$(git -C "$scoped_dir" rev-parse HEAD)" "$scoped_dir"

# Removing an existing leak must not be blocked by the leak it removes.
removal_dir="$(mktemp -d)"
trap 'rm -rf "$removal_dir"' EXIT
setup_repo "$removal_dir"
printf '%s\n' "old: /home/${PERSONAL_ACCOUNT}/thing" >>"$removal_dir/file.txt"
git -C "$removal_dir" add file.txt
git -C "$removal_dir" commit --quiet -m "baseline with a leak"
removal_base="$(git -C "$removal_dir" rev-parse HEAD)"
printf '%s\n' "baseline" >"$removal_dir/file.txt"
git -C "$removal_dir" add file.txt
git -C "$removal_dir" commit --quiet -m "Remove the path"
expect "deleting an existing leak passes" 0 \
  "$removal_base" "$(git -C "$removal_dir" rev-parse HEAD)" "$removal_dir"

if [ "$failures" -gt 0 ]; then
  echo "${failures} self-test case(s) failed"
  exit 1
fi

echo "personal-data guard self-tests passed"
