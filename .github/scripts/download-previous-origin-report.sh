#!/usr/bin/env bash

set -euo pipefail

RUN=""
REPO="${GITHUB_REPOSITORY:-Derek-X-Wang/gmm}"
DIR="previous"

while [ $# -gt 0 ]; do
  case "$1" in
    --run)  RUN="$2"; shift 2 ;;
    --repo) REPO="$2"; shift 2 ;;
    --dir)  DIR="$2"; shift 2 ;;
    *) echo "unrecognised argument: $1" >&2; exit 2 ;;
  esac
done

if [ -z "$RUN" ]; then
  echo "--run is required" >&2
  exit 2
fi

# Querying artifact metadata first is what distinguishes an absent artifact
# (valid first-run state) from a failed download (broken evidence channel).
# `set -e` deliberately propagates API and download failures.
artifact_count="$(gh api \
  "repos/${REPO}/actions/runs/${RUN}/artifacts?per_page=100" \
  --jq '[.artifacts[] | select(.name == "origin-report" and .expired == false)] | length')"

case "$artifact_count" in
  ''|*[!0-9]*)
    echo "invalid artifact count for run $RUN: '$artifact_count'" >&2
    exit 1
    ;;
esac

if [ "$artifact_count" -eq 0 ]; then
  echo "run $RUN published no origin-report artifact"
  exit 0
fi

if [ "$artifact_count" -ne 1 ]; then
  echo "run $RUN published $artifact_count usable origin-report artifacts; refusing ambiguous evidence" >&2
  exit 1
fi

mkdir -p "$DIR"
gh run download "$RUN" --repo "$REPO" --name origin-report --dir "$DIR"

report="${DIR}/origin-report.json"
if [ ! -s "$report" ]; then
  echo "downloaded artifact did not contain a non-empty origin-report.json at '$report'" >&2
  exit 1
fi
echo "downloaded origin report to $report"
