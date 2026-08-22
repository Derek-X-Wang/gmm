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

if [ -z "$CURRENT" ] || [ ! -f "$CURRENT" ]; then
  # No current report means check-origins never ran or died before
  # writing one. That is a broken job, not a healthy manifest, and
  # exiting 0 here would hide it.
  echo "no current report at '${CURRENT}' — cannot decide anything" >&2
  exit 1
fi

# jq that yields one "<game>\t<origin>\t<detail>" line per failing origin.
failing() {
  local file="$1"
  if [ -z "$file" ] || [ ! -f "$file" ]; then
    return 0
  fi
  jq -r '.origins[]? | select(.ok == false)
         | [.game, .origin, (.detail // "")] | @tsv' "$file"
}

# The keys (game + origin) that failed last time, as a lookup.
PREV_KEYS="$(failing "$PREVIOUS" | cut -f1,2 || true)"

ALERTS=""
while IFS=$'\t' read -r game origin detail; do
  [ -n "$game" ] || continue
  if printf '%s\n' "$PREV_KEYS" | grep -qxF "${game}	${origin}"; then
    ALERTS="${ALERTS}${game}	${origin}	${detail}"$'\n'
  else
    echo "first failure for ${game} (${origin}) — not alerting yet"
  fi
done < <(failing "$CURRENT")

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
  body="$(cat <<EOF
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
)"
  echo "opening: ${title}"
  gh issue create --repo "$REPO" \
    --title "$title" \
    --label "$LABEL" \
    --label "bug" \
    --body "$body"
done < <(printf '%s' "$ALERTS")
