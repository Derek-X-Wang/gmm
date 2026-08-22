#!/usr/bin/env bash
#
# Open an issue when a recommended Importer Origin stops resolving.
#
# ADR 0005 relocates staleness detection from the user's screen to the
# maintainer's process. `check-origins` produces the evidence; this
# turns it into an alert — and, just as importantly, declines to.
#
# Two rules, both of which exist so the alert stays worth reading:
#
#   1. Two consecutive failures. Transient GitHub API failures are
#      routine, and an alerting channel that cries wolf is one nobody
#      reads. The "consecutive" memory is the previous scheduled run's
#      report, downloaded as an artifact, so there is no state to keep
#      anywhere.
#   2. One open issue per origin. An origin that stays dead — HIMI is
#      the standing example — must not accumulate a weekly pile.
#
# The alert key is game + origin. Re-pointing a game at a different
# repository is a deliberate act and resets the counter, rather than
# inheriting the previous origin's death.
#
#   dead-origin-issues.sh --current REPORT [--previous REPORT] \
#                         [--repo OWNER/NAME] [--dry-run]
#
# A missing --previous (the first run ever, or an expired artifact) is
# treated as "nothing failed last time", so a first failure never alerts.
# --dry-run prints the alert set and touches no network.

set -euo pipefail

CURRENT=""
PREVIOUS=""
REPO="${GITHUB_REPOSITORY:-Derek-X-Wang/gmm}"
DRY_RUN=0
LABEL="importer-origin-down"

while [ $# -gt 0 ]; do
  case "$1" in
    --current)  CURRENT="$2"; shift 2 ;;
    --previous) PREVIOUS="$2"; shift 2 ;;
    --repo)     REPO="$2"; shift 2 ;;
    --dry-run)  DRY_RUN=1; shift ;;
    *) echo "unrecognised argument: $1" >&2; exit 2 ;;
  esac
done

# Validate a report before deriving failures from it. Do this before entering a
# process substitution: bash does not propagate a producer's status from
# `while ... done < <(...)`, which is how an unreadable report used to become
# an empty (healthy-looking) failure set.
read_failures() {
  local role="$1"
  local file="$2"
  local output="$3"
  local error_file="${PARSE_DIR}/${role}.error"

  if [ -z "$file" ] || [ ! -f "$file" ]; then
    if [ "$role" = "previous" ]; then
      if [ -n "$file" ]; then
        echo "no previous report at '$file'; treating this as the first run"
      else
        echo "no previous report supplied; treating this as the first run"
      fi
      : >"$output"
      return 0
    fi
    echo "no current report at '$file' — cannot decide anything" >&2
    return 1
  fi

  if [ ! -s "$file" ]; then
    echo "could not read ${role} report '$file': file is empty" >&2
    return 1
  fi

  # Parse the whole input before emitting anything. `--slurp` lets this reject
  # both an empty stream and concatenated JSON documents. The counts are
  # independent corroboration written by check-origins: a valid zero-check
  # report is allowed for the ADR-0005 state where every game is retracted,
  # but a bare `{"origins":[]}` is not evidence.
  if ! jq --slurp --raw-output '
    def nonempty_string: type == "string" and length > 0;
    def nonnegative_integer:
      type == "number" and . >= 0 and floor == .;

    if length != 1 then
      error("expected exactly one top-level JSON document")
    else
      .[0] as $report
      | if ($report | type) != "object"
          or ($report.manifest | nonempty_string | not)
          or ($report.checked | nonnegative_integer | not)
          or ($report.failed | nonnegative_integer | not)
          or ($report.origins | type) != "array" then
          error("expected manifest, nonnegative integer checked/failed counts, and an origins array")
        elif all($report.origins[];
          type == "object"
          and (.game | nonempty_string)
          and (.origin | nonempty_string)
          and (.assetPattern | nonempty_string)
          and (.ok | type == "boolean")
          and (.detail | type == "string")
          and ((.ok == false) or (.asset | nonempty_string))) | not then
          error("every verdict must have usable game, origin, assetPattern, ok, detail, and successful asset fields")
        elif $report.checked != ($report.origins | length) then
          error("checked does not match origins length")
        elif $report.failed != ([$report.origins[] | select(.ok == false)] | length) then
          error("failed does not match failing verdicts")
        elif ([$report.origins[] | [.game, .origin]] | unique | length) != $report.checked then
          error("duplicate game and origin alert key")
        else
          $report.origins[]
          | select(.ok == false)
          | [.game, .origin, .detail]
          | @tsv
        end
    end
  ' "$file" >"$output" 2>"$error_file"; then
    echo "could not read ${role} report '$file': $(tr '\n' ' ' < "$error_file")" >&2
    return 1
  fi
}

PARSE_DIR="$(mktemp -d)"
trap 'rm -rf "$PARSE_DIR"' EXIT
CURRENT_FAILURES="${PARSE_DIR}/current.tsv"
PREVIOUS_FAILURES="${PARSE_DIR}/previous.tsv"

read_failures current "$CURRENT" "$CURRENT_FAILURES"
read_failures previous "$PREVIOUS" "$PREVIOUS_FAILURES"

# The keys (game + origin) that failed last time, as a lookup.
PREV_KEYS="$(cut -f1,2 "$PREVIOUS_FAILURES")"

ALERTS=""
while IFS=$'\t' read -r game origin detail; do
  [ -n "$game" ] || continue
  if printf '%s\n' "$PREV_KEYS" | grep -qxF "${game}	${origin}"; then
    ALERTS="${ALERTS}${game}	${origin}	${detail}"$'\n'
  else
    echo "first failure for ${game} (${origin}) — not alerting yet"
  fi
done <"$CURRENT_FAILURES"

if [ -z "${ALERTS//[$'\n\t ']/}" ]; then
  echo "no origin has failed twice in a row; nothing to open"
  exit 0
fi

echo "origins failing for the second consecutive run:"
printf '%s' "$ALERTS"

if [ "$DRY_RUN" = "1" ]; then
  echo "(dry run — no issues created)"
  exit 0
fi

# The label may not exist yet on a fresh repository. Creating it here
# rather than by hand keeps the workflow self-contained.
gh label create "$LABEL" --repo "$REPO" \
  --color B60205 \
  --description "A recommended Importer Origin stopped resolving (ADR 0005)" \
  >/dev/null 2>&1 || true

OPEN_TITLES="$(gh issue list --repo "$REPO" --label "$LABEL" --state open \
  --limit 100 --json title --jq '.[].title')"

while IFS=$'\t' read -r game origin detail; do
  [ -n "$game" ] || continue
  title="Recommended Importer Origin for ${game} stopped resolving: ${origin}"
  if printf '%s\n' "$OPEN_TITLES" | grep -qxF "$title"; then
    echo "already tracked: ${title}"
    continue
  fi
  # Keep the heredoc out of a command substitution for Bash 3.2 portability.
  # GitHub's Bash 4.4+ runner parses the former construct correctly; macOS's
  # system Bash does not.
  body=""
  IFS= read -r -d '' body <<EOF || true
> *Opened automatically by the scheduled \`upstream importers\` workflow.*

The Importer Origin GMM recommends for **${game}** has failed to resolve on
two consecutive scheduled runs.

| | |
|---|---|
| Game | \`${game}\` |
| Origin | [\`${origin}\`](https://github.com/${origin}/releases/latest) |
| Failure | \`${detail}\` |

## Why this matters

Every released build fetches
[\`manifest/recommended-importers.json\`](https://github.com/${REPO}/blob/main/manifest/recommended-importers.json)
at runtime, so whatever this file says is what users are pointed at
right now. Per ADR 0005 GMM is a conduit, not a maintainer — the fix is
to change where the manifest points, not to adopt the package.

## What to do

- If upstream renamed its release asset, widen or correct that game's \`assetPattern\`.
- If upstream moved, point \`owner\`/\`repo\` at the new home.
- If no maintained package exists any more, set the entry to \`"status": "none"\`
  with a \`reason\`. That **retracts** the compiled-in default rather than
  falling through to it — see \`manifest/README.md\`.

Validate before merging: \`cd src-tauri && cargo run --bin validate-manifest\`.

This issue is not reopened or duplicated while it stays open. Close it once
the manifest is fixed; the next scheduled run will re-open one if it is not.
EOF
  echo "opening: ${title}"
  gh issue create --repo "$REPO" \
    --title "$title" \
    --label "$LABEL" \
    --label "bug" \
    --body "$body"
done < <(printf '%s' "$ALERTS")
