#!/usr/bin/env bash

# This repository is public. Commit messages and diffs are permanent and
# world-readable, and the rule against putting personal data in them has
# until now lived only as prose in CLAUDE.md. Prose asks the next author to
# remember; this asks the build.
#
# Scope is the pull request range only. Published history is deliberately
# untouched: a rewrite would diverge every clone and still leave the old
# objects reachable by SHA, so the gate stops the class from growing rather
# than pretending to erase it.

set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 BASE_COMMIT HEAD_COMMIT" >&2
  exit 2
fi

BASE_COMMIT="$1"
HEAD_COMMIT="$2"

git rev-parse --verify --quiet "${BASE_COMMIT}^{commit}" >/dev/null || {
  echo "could not resolve pull request base commit: ${BASE_COMMIT}" >&2
  exit 2
}
git rev-parse --verify --quiet "${HEAD_COMMIT}^{commit}" >/dev/null || {
  echo "could not resolve pull request head commit: ${HEAD_COMMIT}" >&2
  exit 2
}

# Home-directory path segments that name a role rather than a person. A CI
# runner's own path is not a leak; a contributor's account name is. Anything
# not listed here is treated as a real account, so the gate fails closed on
# names it has never seen.
IMPERSONAL_ACCOUNTS='^(runner|user|username|USERNAME|you|someone|example|test|ci|root|Public|Shared|All Users|Default)$'

failures=0

report() {
  local where="$1" what="$2" evidence="$3"
  echo "::error::${where} contains ${what}."
  printf '%s\n' "$evidence" | sed 's/^/    /'
  failures=$((failures + 1))
}

scan_text() {
  local where="$1" text="$2" hits account path_hits=""

  hits="$(printf '%s\n' "$text" \
    | grep -oE 'Claude-Session:[^ ]*|claude\.ai/code/session_[A-Za-z0-9]+' \
    | sort -u || true)"
  if [ -n "$hits" ]; then
    report "$where" "a Claude session link" "$hits"
  fi

  # Match the account segment so the decision can be made on the name. A bare
  # /Users/ or /home/ prefix is not itself evidence of anything. Windows
  # paths are matched in raw and source-escaped form alike, because a leak
  # inside a string literal is spelled with doubled separators and would
  # otherwise slip past a pattern written for the raw spelling.
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    account="${hit##*[/\\]}"
    if ! printf '%s' "$account" | grep -qE "$IMPERSONAL_ACCOUNTS"; then
      path_hits="${path_hits}${hit}"$'\n'
    fi
  done < <(printf '%s\n' "$text" \
    | grep -oE '(/Users/|/home/|[A-Za-z]:\\{1,2}Users\\{1,2})[A-Za-z0-9._-]+' \
    | sort -u || true)

  if [ -n "$path_hits" ]; then
    report "$where" "a home-directory path naming an account" "${path_hits%$'\n'}"
  fi
}

while IFS= read -r sha; do
  [ -n "$sha" ] || continue
  scan_text "commit $(git rev-parse --short "$sha") message" \
    "$(git log -1 --format='%B' "$sha")"
done < <(git rev-list "${BASE_COMMIT}..${HEAD_COMMIT}")

# Only added lines. A pull request that deletes an existing leak must pass.
scan_text "an added line in this pull request" \
  "$(git diff --no-color --unified=0 "$BASE_COMMIT" "$HEAD_COMMIT" \
     | grep '^+' | grep -v '^+++' || true)"

if [ "$failures" -gt 0 ]; then
  echo
  echo "This repository is public. Rewrite the commit message or the line so it"
  echo "carries no session link and no path naming a personal account."
  echo "Add the account to IMPERSONAL_ACCOUNTS only if it names a role, not a person."
  exit 1
fi

echo "No session links or personal home-directory paths in this pull request."
