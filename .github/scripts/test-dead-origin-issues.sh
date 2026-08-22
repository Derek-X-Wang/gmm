#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALERTER="${SCRIPT_DIR}/dead-origin-issues.sh"
DOWNLOADER="${SCRIPT_DIR}/download-previous-origin-report.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

pass=0
fail=0

record_pass() {
  echo "ok - $1"
  pass=$((pass + 1))
}

record_fail() {
  echo "not ok - $1" >&2
  echo "  $2" >&2
  fail=$((fail + 1))
}

run_command() {
  local output_file="$1"
  shift

  set +e
  "$@" >"$output_file" 2>&1
  COMMAND_STATUS=$?
  set -e
}

expect_success() {
  local name="$1"
  local pattern="$2"
  shift 2
  local output="$TMP/output-${pass}-${fail}.txt"

  run_command "$output" "$@"
  if [ "$COMMAND_STATUS" -eq 0 ] && grep -qF "$pattern" "$output"; then
    record_pass "$name"
  else
    record_fail "$name" "expected success containing '$pattern'; status=$COMMAND_STATUS; output=$(tr '\n' ' ' < "$output")"
  fi
}

expect_failure() {
  local name="$1"
  local pattern="$2"
  shift 2
  local output="$TMP/output-${pass}-${fail}.txt"

  run_command "$output" "$@"
  if [ "$COMMAND_STATUS" -ne 0 ] && grep -qF "$pattern" "$output"; then
    record_pass "$name"
  else
    record_fail "$name" "expected failure containing '$pattern'; status=$COMMAND_STATUS; output=$(tr '\n' ' ' < "$output")"
  fi
}

printf '%s\n' 'not json' >"$TMP/malformed-current.json"
printf '%s\n' '{"origins":' >"$TMP/malformed-previous.json"
printf '%s\n' '{"origins":[]}' >"$TMP/empty.json"
printf '%s\n' '{"origins":[{"game":"himi","origin":"a/b","ok":false,"detail":"gone"}]}' >"$TMP/failing.json"

expect_failure \
  "malformed current report is a broken job" \
  "could not read current report '$TMP/malformed-current.json'" \
  "$ALERTER" --current "$TMP/malformed-current.json" --dry-run

expect_failure \
  "malformed previous report is not a first failure" \
  "could not read previous report '$TMP/malformed-previous.json'" \
  "$ALERTER" --current "$TMP/failing.json" --previous "$TMP/malformed-previous.json" --dry-run

expect_failure \
  "a report with no origins is implausible" \
  "contains no origins" \
  "$ALERTER" --current "$TMP/empty.json" --dry-run

expect_success \
  "an absent previous report remains a first failure" \
  "no previous report at '$TMP/absent.json'; treating this as the first run" \
  "$ALERTER" --current "$TMP/failing.json" --previous "$TMP/absent.json" --dry-run

expect_success \
  "a second consecutive failure alerts" \
  "origins failing for the second consecutive run:" \
  "$ALERTER" --current "$TMP/failing.json" --previous "$TMP/failing.json" --dry-run

mkdir -p "$TMP/bin"
cp "$SCRIPT_DIR/test-fixtures/fake-gh.sh" "$TMP/bin/gh"
chmod +x "$TMP/bin/gh"

expect_success \
  "an existing open issue is not duplicated" \
  "already tracked: Recommended Importer Origin for himi stopped resolving: a/b" \
  env PATH="$TMP/bin:$PATH" FAKE_GH_MODE=existing-issue \
  "$ALERTER" --current "$TMP/failing.json" --previous "$TMP/failing.json"

expect_success \
  "a previous run with no artifact is an absent report" \
  "run 42 published no origin-report artifact" \
  env PATH="$TMP/bin:$PATH" FAKE_GH_MODE=no-artifact \
  "$DOWNLOADER" --run 42 --repo Derek-X-Wang/gmm --dir "$TMP/previous-none"

expect_failure \
  "an artifact download failure fails loudly" \
  "simulated download failure" \
  env PATH="$TMP/bin:$PATH" FAKE_GH_MODE=download-failure \
  "$DOWNLOADER" --run 42 --repo Derek-X-Wang/gmm --dir "$TMP/previous-failed"

echo "$pass passed; $fail failed"
[ "$fail" -eq 0 ]
