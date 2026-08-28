#!/usr/bin/env bash

# This repository is public. Commit messages and diffs are permanent and
# world-readable, and the rule against putting personal data in them has until
# now lived only as prose in CLAUDE.md. Prose asks the next author to remember;
# this asks the build.
#
# Scope, stated plainly because it is narrower than the repository rule:
#
#   covered      commit messages in the pull request range, and lines the range
#                adds to text files
#   not covered  pull request titles and bodies, issue and review comments,
#                binary or UTF-16 additions, a path split across two source
#                lines, and every kind of personal data other than a session
#                link or a home-directory path
#
# Published history is deliberately untouched. A rewrite would diverge every
# clone and still leave the old objects reachable by SHA, so this stops the
# class from growing rather than pretending to erase it.

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

# Home-directory segments that name a role rather than a person. A CI runner's
# own path is not a leak; a contributor's account name is. Anything not listed
# here is treated as a real account, so the gate fails closed on names it has
# never seen. Matched case-insensitively.
#
# Entries containing a space cannot appear: the extractor stops at the first
# space, so only single-token names are reachable.
IMPERSONAL_ACCOUNTS='^(runner|runneradmin|user|username|you|someone|example|test|ci|root|public|shared|default)$'

# This guard and its self-test necessarily contain specimens of the thing they
# forbid: the pattern names the trailer, and the self-test has to feed real
# leaks to a real checker. Scanning them would make the gate reject itself. The
# exemption is by exact path and covers nothing else, so a leak anywhere in the
# repository is still caught even in the same pull request that edits these two
# files. It does leave one blind spot -- a leak hidden inside them -- which is
# narrow, deliberate, and stated here rather than discovered later.
EXCLUSIONS=(
  ':(exclude).github/scripts/check-no-personal-data.sh'
  ':(exclude).github/scripts/test-no-personal-data.sh'
)

failures=0

report() {
  local where="$1" what="$2" evidence="$3"
  echo "::error::${where} contains ${what}."
  printf '%s\n' "$evidence" | sed 's/^/    /'
  failures=$((failures + 1))
}

# grep exits 1 for "no match" and 2 or more for a real failure. Collapsing
# those together is the same mistake this whole gate exists to prevent, so an
# operational failure aborts rather than reading as "nothing found".
#
# The result is published through a global rather than returned. A function
# called inside $(...) runs in a subshell, where `exit 2` ends only the
# subshell and the caller carries on with an empty string -- the failure this
# very handler exists to prevent.
MATCHES=""
match_all() {
  local pattern="$1" text="$2" out status

  set +e
  out="$(printf '%s\n' "$text" | grep -oEi "$pattern" | sort -u)"
  status=$?
  set -e

  if [ "$status" -gt 1 ]; then
    echo "grep failed while scanning (status ${status})" >&2
    exit 2
  fi
  MATCHES="$out"
}

# Fail closed on a grep that broke rather than found nothing, as above.
filter_lines() {
  local text="$1" out status
  shift
  set +e
  out="$(printf '%s\n' "$text" | "$@")"
  status=$?
  set -e
  if [ "$status" -gt 1 ]; then
    echo "grep failed while selecting added lines (status ${status})" >&2
    exit 2
  fi
  FILTERED="$out"
}

scan_text() {
  local where="$1" text="$2" hits account path_hits=""

  match_all 'Claude-Session:[^ ]*|claude\.ai/code/session_[A-Za-z0-9]+' "$text"
  hits="$MATCHES"
  if [ -n "$hits" ]; then
    report "$where" "a Claude session link" "$hits"
  fi

  # Match the account segment so the decision can be made on the name; a bare
  # /Users/ or /home/ prefix is not itself evidence of anything.
  #
  # Separators are matched in every spelling a real file uses: a raw slash, a
  # backslash, the doubled backslash of a source string literal, and the
  # backslash-escaped slash of serialized JSON.
  #
  # The leading boundary keeps a URL or a package import from reading as a home
  # directory: in https://example.com/users/alice the segment is preceded by a
  # word character, so it is part of a longer token rather than a path.
  #
  # A known false positive survives: prose such as "/home/directories" is
  # indistinguishable from a real path, and IMPERSONAL_ACCOUNTS is the intended
  # escape hatch for it.
  match_all '(^|[^A-Za-z0-9._-])((\\?/)(users|home)(\\?/)|[A-Za-z]:\\{1,2}users\\{1,2})[A-Za-z0-9._-]+' "$text"
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    account="${hit##*[/\\]}"
    if ! printf '%s' "$account" | grep -qiE "$IMPERSONAL_ACCOUNTS"; then
      path_hits="${path_hits}${hit}"$'\n'
    fi
  done <<<"$MATCHES"

  if [ -n "$path_hits" ]; then
    report "$where" "a home-directory path naming an account" "${path_hits%$'\n'}"
  fi
}

# Every commit message in the range, not just the tip. This repository squash
# merges, and GitHub folds the individual messages into the squashed commit
# body, so a trailer written on any commit reaches main -- which is how the
# session trailers already in published history got there.
commits="$(git rev-list "${BASE_COMMIT}..${HEAD_COMMIT}")" || {
  echo "could not enumerate commits in ${BASE_COMMIT}..${HEAD_COMMIT}" >&2
  exit 2
}

while IFS= read -r sha; do
  [ -n "$sha" ] || continue
  message="$(git log -1 --format='%B' "$sha")" || {
    echo "could not read the message of ${sha}" >&2
    exit 2
  }
  scan_text "commit $(git rev-parse --short "$sha") message" "$message"
done <<<"$commits"

# Added lines only, so a pull request that deletes an existing leak passes.
#
# This is the net range diff rather than a walk of each commit's diff, which is
# correct under squash merge: a line added and then removed within the pull
# request never reaches main, so it is not a leak to report.
diff_output="$(git diff --no-color --unified=0 \
  "$BASE_COMMIT" "$HEAD_COMMIT" -- . "${EXCLUSIONS[@]}")" || {
  echo "could not diff ${BASE_COMMIT}..${HEAD_COMMIT}" >&2
  exit 2
}

# The file header is '+++ ' with a trailing space. Matching '+++' alone would
# also discard a genuine added line whose own text begins with '++'.
FILTERED=""
filter_lines "$diff_output" grep '^+'
filter_lines "$FILTERED" grep -v '^+++ '
added="$FILTERED"
scan_text "an added line in this pull request" "$added"

if [ "$failures" -gt 0 ]; then
  echo
  echo "This repository is public. Rewrite the commit message or the line so it"
  echo "carries no session link and no path naming a personal account."
  echo "Add the account to IMPERSONAL_ACCOUNTS only if it names a role, not a person."
  exit 1
fi

echo "No session links or personal home-directory paths in this pull request."
