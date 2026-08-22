#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALERTER="${SCRIPT_DIR}/dead-origin-issues.sh"
DOWNLOADER="${SCRIPT_DIR}/download-previous-origin-report.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

pass=0
fail=0
EXPECTED_TESTS=30

record_pass() {
  echo "ok - $1"
  pass=$((pass + 1))
}

record_fail() {
  echo "not ok - $1" >&2
  echo "  $2" >&2
  fail=$((fail + 1))
}

expect_run() {
  local name="$1"
  local expectation="$2"
  local required="$3"
  local forbidden="$4"
  shift 4
  local output="$TMP/output-${pass}-${fail}.txt"
  local status_ok=0
  local required_ok=0
  local forbidden_ok=0

  set +e
  "$@" >"$output" 2>&1
  local command_status=$?
  set -e

  case "$expectation:$command_status" in
    success:0|failure:[1-9]*|failure:[1-9]*[0-9]*) status_ok=1 ;;
  esac
  if [ -z "$required" ] || grep -qF "$required" "$output"; then
    required_ok=1
  fi
  if [ -z "$forbidden" ] || ! grep -qF "$forbidden" "$output"; then
    forbidden_ok=1
  fi

  if [ "$status_ok" -eq 1 ] && [ "$required_ok" -eq 1 ] && [ "$forbidden_ok" -eq 1 ]; then
    record_pass "$name"
  else
    record_fail "$name" "expected=$expectation required='$required' forbidden='$forbidden'; status=$command_status; output=$(tr '\n' ' ' < "$output")"
  fi
}

expect_call_counts() {
  local name="$1"
  local log="$2"
  local expected_labels="$3"
  local expected_lists="$4"
  local expected_creates="$5"
  local labels lists creates

  labels="$(awk '$0 == "label.create" { count++ } END { print count + 0 }' "$log")"
  lists="$(awk '$0 == "issue.list" { count++ } END { print count + 0 }' "$log")"
  creates="$(awk '$0 == "issue.create" { count++ } END { print count + 0 }' "$log")"
  if [ "$labels" -eq "$expected_labels" ] &&
     [ "$lists" -eq "$expected_lists" ] &&
     [ "$creates" -eq "$expected_creates" ]; then
    record_pass "$name"
  else
    record_fail "$name" "expected calls label/list/create=$expected_labels/$expected_lists/$expected_creates; actual=$labels/$lists/$creates"
  fi
}

expect_nonempty_file() {
  local name="$1"
  local file="$2"
  if [ -s "$file" ]; then
    record_pass "$name"
  else
    record_fail "$name" "expected a non-empty file at $file"
  fi
}

VALID_FAILURE='{"manifest":"manifest/recommended-importers.json","checked":1,"failed":1,"origins":[{"game":"himi","origin":"a/b","assetPattern":"HIMI.*zip","ok":false,"detail":"gone"}]}'
VALID_HEALTHY='{"manifest":"manifest/recommended-importers.json","checked":1,"failed":0,"origins":[{"game":"gimi","origin":"c/d","assetPattern":"GIMI.*zip","ok":true,"asset":"GIMI.zip","detail":"selected GIMI.zip"}]}'
VALID_EMPTY='{"manifest":"manifest/recommended-importers.json","checked":0,"failed":0,"origins":[]}'

printf '%s\n' 'not json' >"$TMP/malformed-current.json"
printf '%s\n' '{"origins":' >"$TMP/malformed-previous.json"
: >"$TMP/zero-byte-current.json"
: >"$TMP/zero-byte-previous.json"
printf '%s\n' '{"origins":[]}' >"$TMP/bare-empty.json"
printf '%s\n' '{"manifest":"m","checked":1,"failed":0,"origins":[{}]}' >"$TMP/empty-verdict.json"
printf '%s\n' '{"manifest":"m","checked":1,"failed":0,"origins":[null]}' >"$TMP/null-verdict.json"
printf '%s\n' '{"manifest":"m","checked":1,"failed":0,"origins":[{"game":"himi","origin":"a/b","assetPattern":"x","ok":"false","detail":"gone"}]}' >"$TMP/string-ok.json"
printf '%s\n' '{"checked":3,"failed":2,"summary":"two failed"}' >"$TMP/summary-only.json"
printf '%s\n' '{"manifest":"m","checked":1,"failed":1,"origins":[{"game":"himi","origin":"a/b","ok":false,"detail":"gone"}]}' >"$TMP/missing-pattern.json"
printf '%s\n' '{"manifest":"m","checked":1,"failed":1,"origins":[{"game":"himi","origin":"a/b","assetPattern":"x","ok":false}]}' >"$TMP/missing-detail.json"
printf '%s\n' '{"manifest":"m","checked":1,"failed":0,"origins":[{"game":"himi","origin":"a/b","assetPattern":"x","ok":true,"detail":"selected x"}]}' >"$TMP/missing-asset.json"
printf '%s\n' '{"manifest":"m","checked":2,"failed":1,"origins":[{"game":"himi","origin":"a/b","assetPattern":"x","ok":false,"detail":"gone"}]}' >"$TMP/checked-mismatch.json"
printf '%s\n' '{"manifest":"m","checked":1,"failed":0,"origins":[{"game":"himi","origin":"a/b","assetPattern":"x","ok":false,"detail":"gone"}]}' >"$TMP/failed-mismatch.json"
printf '%s\n' '{"manifest":"m","checked":2,"failed":2,"origins":[{"game":"himi","origin":"a/b","assetPattern":"x","ok":false,"detail":"gone"},{"game":"himi","origin":"a/b","assetPattern":"x","ok":false,"detail":"still gone"}]}' >"$TMP/duplicate.json"
printf '%s\n' '{"manifest":"m","checked":"1","failed":0,"origins":[]}' >"$TMP/string-count.json"
printf '%s\n%s\n' "$VALID_FAILURE" "$VALID_FAILURE" >"$TMP/multiple-documents.json"
printf '%s\n' "$VALID_FAILURE" >"$TMP/failing.json"
printf '%s\n' "$VALID_HEALTHY" >"$TMP/healthy.json"
printf '%s\n' "$VALID_EMPTY" >"$TMP/valid-empty.json"

expect_run "malformed current report is a broken job" failure \
  "could not read current report '$TMP/malformed-current.json'" "" \
  "$ALERTER" --current "$TMP/malformed-current.json" --dry-run

expect_run "malformed previous report is not a first failure" failure \
  "could not read previous report '$TMP/malformed-previous.json'" "first failure" \
  "$ALERTER" --current "$TMP/failing.json" --previous "$TMP/malformed-previous.json" --dry-run

expect_run "zero-byte current report is unreadable" failure \
  "could not read current report '$TMP/zero-byte-current.json'" "nothing to open" \
  "$ALERTER" --current "$TMP/zero-byte-current.json" --dry-run

expect_run "zero-byte previous report cannot erase history" failure \
  "could not read previous report '$TMP/zero-byte-previous.json'" "first failure" \
  "$ALERTER" --current "$TMP/failing.json" --previous "$TMP/zero-byte-previous.json" --dry-run

expect_run "bare empty origins lacks corroborating counts" failure \
  "could not read current report '$TMP/bare-empty.json'" "nothing to open" \
  "$ALERTER" --current "$TMP/bare-empty.json" --dry-run

for malformed_case in empty-verdict null-verdict string-ok summary-only missing-pattern missing-detail missing-asset; do
  expect_run "$malformed_case report is unusable" failure \
    "could not read current report '$TMP/$malformed_case.json'" "nothing to open" \
    "$ALERTER" --current "$TMP/$malformed_case.json" --dry-run
done

expect_run "checked must equal the number of verdicts" failure \
  "checked does not match origins length" "nothing to open" \
  "$ALERTER" --current "$TMP/checked-mismatch.json" --dry-run

expect_run "failed must equal the false verdicts" failure \
  "failed does not match failing verdicts" "nothing to open" \
  "$ALERTER" --current "$TMP/failed-mismatch.json" --dry-run

expect_run "alert keys must be unique" failure \
  "duplicate game and origin alert key" "nothing to open" \
  "$ALERTER" --current "$TMP/duplicate.json" --dry-run

expect_run "checked and failed must be integers" failure \
  "nonnegative integer checked/failed counts" "nothing to open" \
  "$ALERTER" --current "$TMP/string-count.json" --dry-run

expect_run "exactly one top-level document is required" failure \
  "expected exactly one top-level JSON document" "nothing to open" \
  "$ALERTER" --current "$TMP/multiple-documents.json" --dry-run

expect_run "zero checked origins is valid when the counts corroborate it" success \
  "nothing to open" "could not read" \
  "$ALERTER" --current "$TMP/valid-empty.json" --dry-run

expect_run "a valid healthy verdict remains an all-clear" success \
  "nothing to open" "could not read" \
  "$ALERTER" --current "$TMP/healthy.json" --dry-run

mkdir -p "$TMP/bin"
cp "$SCRIPT_DIR/test-fixtures/fake-gh.sh" "$TMP/bin/gh"
chmod +x "$TMP/bin/gh"

first_log="$TMP/first-calls.log"
: >"$first_log"
expect_run "an absent previous report makes exactly a first-failure decision" success \
  "first failure for himi (a/b) — not alerting yet" "origins failing for the second consecutive run" \
  env PATH="$TMP/bin:$PATH" FAKE_GH_MODE=forbid FAKE_GH_LOG="$first_log" \
  "$ALERTER" --current "$TMP/failing.json" --previous "$TMP/absent.json"

open_log="$TMP/open-calls.log"
: >"$open_log"
expect_run "a second consecutive failure opens an issue" success \
  "opening: Recommended Importer Origin for himi stopped resolving: a/b" "already tracked" \
  env PATH="$TMP/bin:$PATH" FAKE_GH_MODE=open-issue FAKE_GH_LOG="$open_log" \
  "$ALERTER" --current "$TMP/failing.json" --previous "$TMP/failing.json"
expect_call_counts "a second failure creates exactly one issue" "$open_log" 1 1 1

dedupe_log="$TMP/dedupe-calls.log"
: >"$dedupe_log"
expect_run "an existing open issue is not duplicated" success \
  "already tracked: Recommended Importer Origin for himi stopped resolving: a/b" "opening:" \
  env PATH="$TMP/bin:$PATH" FAKE_GH_MODE=existing-issue FAKE_GH_LOG="$dedupe_log" \
  "$ALERTER" --current "$TMP/failing.json" --previous "$TMP/failing.json"
expect_call_counts "deduplication creates zero issues" "$dedupe_log" 1 1 0

expect_run "a previous run with no artifact is an absent report" success \
  "run 42 published no origin-report artifact" "" \
  env PATH="$TMP/bin:$PATH" FAKE_GH_MODE=no-artifact FAKE_GH_LOG="$TMP/no-artifact.log" \
  "$DOWNLOADER" --run 42 --repo Derek-X-Wang/gmm --dir "$TMP/previous-none"

expect_run "a non-numeric artifact count is rejected" failure \
  "invalid artifact count" "published no origin-report" \
  env PATH="$TMP/bin:$PATH" FAKE_GH_MODE=invalid-count FAKE_GH_LOG="$TMP/invalid-count.log" \
  "$DOWNLOADER" --run 42 --repo Derek-X-Wang/gmm --dir "$TMP/previous-invalid-count"

expect_run "an artifact download failure fails loudly" failure \
  "simulated download failure" "" \
  env PATH="$TMP/bin:$PATH" FAKE_GH_MODE=download-failure FAKE_GH_LOG="$TMP/download-failure.log" \
  FAKE_GH_EXPECTED_DIR="$TMP/previous-failed" \
  "$DOWNLOADER" --run 42 --repo Derek-X-Wang/gmm --dir "$TMP/previous-failed"

expect_run "a successful download that writes nothing is rejected" failure \
  "downloaded artifact did not contain a non-empty origin-report.json" "" \
  env PATH="$TMP/bin:$PATH" FAKE_GH_MODE=download-empty FAKE_GH_LOG="$TMP/download-empty.log" \
  FAKE_GH_EXPECTED_DIR="$TMP/previous-empty" \
  "$DOWNLOADER" --run 42 --repo Derek-X-Wang/gmm --dir "$TMP/previous-empty"

expect_run "a successful artifact download confirms the report arrived" success \
  "downloaded origin report to $TMP/previous-success/origin-report.json" "" \
  env PATH="$TMP/bin:$PATH" FAKE_GH_MODE=download-success FAKE_GH_LOG="$TMP/download-success.log" \
  FAKE_GH_EXPECTED_DIR="$TMP/previous-success" \
  "$DOWNLOADER" --run 42 --repo Derek-X-Wang/gmm --dir "$TMP/previous-success"
expect_nonempty_file "the successful download produced non-empty evidence" \
  "$TMP/previous-success/origin-report.json"

total=$((pass + fail))
echo "$pass passed; $fail failed; $total executed"
if [ "$total" -ne "$EXPECTED_TESTS" ]; then
  echo "not ok - expected exactly $EXPECTED_TESTS tests, executed $total" >&2
  exit 1
fi
[ "$fail" -eq 0 ]
