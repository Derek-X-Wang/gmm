#!/usr/bin/env bash

set -euo pipefail

MODE="${FAKE_GH_MODE:-}"
LOG="${FAKE_GH_LOG:-}"
REPO="Derek-X-Wang/gmm"
ARTIFACT_FILTER='[.artifacts[] | select(.name == "origin-report" and .expired == false)] | length'

die() {
  echo "unexpected fake gh call: mode=${MODE:-unset} args=$*" >&2
  exit 98
}

expect_args() {
  local expected_count="$1"
  shift
  local combined_count=$((expected_count * 2))
  [ "$#" -eq "$combined_count" ] ||
    die "expected $expected_count actual and $expected_count contract arguments, received $# ($*)"
  local actual=("${@:1:expected_count}")
  shift "$expected_count"
  local expected=("$@")
  local index=0
  while [ "$index" -lt "$expected_count" ]; do
    [ "${actual[$index]}" = "${expected[$index]}" ] ||
      die "argument $((index + 1)): expected '${expected[$index]}', received '${actual[$index]}'"
    index=$((index + 1))
  done
}

mark() {
  [ -n "$LOG" ] || die "FAKE_GH_LOG is required"
  printf '%s\n' "$1" >>"$LOG"
}

expect_api() {
  expect_args 4 "$@" \
    api "repos/${REPO}/actions/runs/42/artifacts?per_page=100" \
    --jq "$ARTIFACT_FILTER"
  mark api
}

expect_download() {
  local expected_dir="${FAKE_GH_EXPECTED_DIR:-}"
  [ -n "$expected_dir" ] || die "FAKE_GH_EXPECTED_DIR is required"
  expect_args 9 "$@" \
    run download 42 --repo "$REPO" --name origin-report --dir "$expected_dir"
  mark run.download
}

expect_label_create() {
  expect_args 9 "$@" \
    label create importer-origin-down \
    --repo "$REPO" \
    --color B60205 \
    --description "A recommended Importer Origin stopped resolving (ADR 0005)"
  mark label.create
}

expect_issue_list() {
  expect_args 14 "$@" \
    issue list \
    --repo "$REPO" \
    --label importer-origin-down \
    --state open \
    --limit 100 \
    --json title \
    --jq '.[].title'
  mark issue.list
}

expect_issue_create() {
  local expected_game_row="| Game | \`himi\` |"
  [ "$#" -eq 12 ] || die "expected 12 issue-create arguments, received $# ($*)"
  [ "$1" = issue ] || die "$@"
  [ "$2" = create ] || die "$@"
  [ "$3" = --repo ] && [ "$4" = "$REPO" ] || die "$@"
  [ "$5" = --title ] &&
    [ "$6" = "Recommended Importer Origin for himi stopped resolving: a/b" ] || die "$@"
  [ "$7" = --label ] && [ "$8" = importer-origin-down ] || die "$@"
  [ "$9" = --label ] && [ "${10}" = bug ] || die "$@"
  [ "${11}" = --body ] || die "$@"
  case "${12}" in
    *"$expected_game_row"*a/b*gone*) ;;
    *) die "issue body did not carry the game, origin, and failure detail" ;;
  esac
  mark issue.create
}

case "$MODE:$1:$2" in
  existing-issue:label:create|open-issue:label:create)
    expect_label_create "$@"
    ;;
  existing-issue:issue:list)
    expect_issue_list "$@"
    echo "Recommended Importer Origin for himi stopped resolving: a/b"
    ;;
  open-issue:issue:list)
    expect_issue_list "$@"
    ;;
  existing-issue:issue:create)
    die "issue creation must not be reached when the alert is already open"
    ;;
  open-issue:issue:create)
    expect_issue_create "$@"
    ;;
  no-artifact:api:*|invalid-count:api:*|download-failure:api:*|download-empty:api:*|download-success:api:*)
    expect_api "$@"
    case "$MODE" in
      no-artifact) echo 0 ;;
      invalid-count) echo not-a-number ;;
      *) echo 1 ;;
    esac
    ;;
  download-failure:run:download)
    expect_download "$@"
    echo "simulated download failure" >&2
    exit 42
    ;;
  download-empty:run:download)
    expect_download "$@"
    ;;
  download-success:run:download)
    expect_download "$@"
    printf '%s\n' '{"manifest":"m","checked":0,"failed":0,"origins":[]}' \
      >"${FAKE_GH_EXPECTED_DIR}/origin-report.json"
    ;;
  *)
    die "$@"
    ;;
esac
