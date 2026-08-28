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

# A mutation keeping only the first sorted match would report the allowlisted
# path and drop the real one, since /home/example sorts before /home/<account>.
run_case "a personal path after an allowlisted one fails" 1 \
  "Fix the thing" "vendor/thirdparty/notes.md" \
  "cp /home/example/a /home/${PERSONAL_ACCOUNT}/b"

# Fixtures above cluster in src/, docs/ and src-tauri/. These live where a
# pathspec narrowed to those trees would stop looking.
run_case "a leak at the repository root fails" 1 \
  "Fix the thing" "README.md" "built from /home/${PERSONAL_ACCOUNT}/gmm"

run_case "a leak under .github fails" 1 \
  "Fix the thing" ".github/workflows/local.yml" \
  "working-directory: /home/${PERSONAL_ACCOUNT}/gmm"

run_case "a leak under public fails" 1 \
  "Fix the thing" "public/config.json" \
  "{\"root\": \"/home/${PERSONAL_ACCOUNT}\"}"

# A gate that flags ordinary URLs and package imports gets switched off.
run_case "a URL path segment passes" 0 \
  "Fix the thing" "docs/links.md" "see https://example.com/users/alice"

run_case "a package import path passes" 0 \
  "Fix the thing" "src/deps.go" 'import "example.com/home/alice/pkg"'

# Every case above puts its leak in the single commit at the tip, so a checker
# reading only the newest message would still pass them all. Here the leak is
# in an earlier commit and a clean commit follows it.
buried_dir="$(mktemp -d)"
setup_repo "$buried_dir"
buried_base="$(git -C "$buried_dir" rev-parse HEAD)"
printf '%s\n' "first change" >>"$buried_dir/src/main.rs"
git -C "$buried_dir" add src/main.rs
git -C "$buried_dir" commit --quiet -m "Fix the first thing

Claude-Session: https://claude.ai/code/session_BURIED0123456789"
printf '%s\n' "second change" >>"$buried_dir/docs/notes.md"
git -C "$buried_dir" add docs/notes.md
git -C "$buried_dir" commit --quiet -m "Fix the second thing"
expect "a leak in an earlier commit message is caught" 1 \
  "$buried_base" "$(git -C "$buried_dir" rev-parse HEAD)" "$buried_dir"
rm -rf "$buried_dir"

# The same, for a home-directory path rather than a session link.
buried_path_dir="$(mktemp -d)"
setup_repo "$buried_path_dir"
buried_path_base="$(git -C "$buried_path_dir" rev-parse HEAD)"
printf '%s\n' "first change" >>"$buried_path_dir/src/main.rs"
git -C "$buried_path_dir" add src/main.rs
git -C "$buried_path_dir" commit --quiet -m "Fix the first thing

Reproduced from /home/${PERSONAL_ACCOUNT}/gmm."
printf '%s\n' "second change" >>"$buried_path_dir/docs/notes.md"
git -C "$buried_path_dir" add docs/notes.md
git -C "$buried_path_dir" commit --quiet -m "Fix the second thing"
expect "a path in an earlier commit message is caught" 1 \
  "$buried_path_base" "$(git -C "$buried_path_dir" rev-parse HEAD)" "$buried_path_dir"
rm -rf "$buried_path_dir"

# Linear history does not prove that every commit reachable through a merge is
# scanned. Put the leak only in the second parent's message; neither the merge
# commit nor the first-parent chain carries it.
merged_dir="$(mktemp -d)"
setup_repo "$merged_dir"
merged_base="$(git -C "$merged_dir" rev-parse HEAD)"
primary_branch="$(git -C "$merged_dir" branch --show-current)"
git -C "$merged_dir" switch --quiet -c imported-work
printf '%s\n' "a harmless imported change" >"$merged_dir/docs/imported.md"
git -C "$merged_dir" add docs/imported.md
git -C "$merged_dir" commit --quiet -m "Import the change

Claude-Session: https://claude.ai/code/session_SECOND_PARENT0123456789"
git -C "$merged_dir" switch --quiet "$primary_branch"
printf '%s\n' "a harmless first-parent change" >>"$merged_dir/src/main.rs"
git -C "$merged_dir" add src/main.rs
git -C "$merged_dir" commit --quiet -m "Prepare the main line"
git -C "$merged_dir" merge --quiet --no-ff imported-work -m "Merge imported work"
expect "a leak in a second-parent commit message is caught" 1 \
  "$merged_base" "$(git -C "$merged_dir" rev-parse HEAD)" "$merged_dir"
rm -rf "$merged_dir"

# The merge above has a clean merge message, and every other message fixture
# puts its leak below the subject. A checker skipping merge commits or reading
# only message bodies would therefore still pass the suite.
merge_message_dir="$(mktemp -d)"
setup_repo "$merge_message_dir"
merge_message_base="$(git -C "$merge_message_dir" rev-parse HEAD)"
primary_branch="$(git -C "$merge_message_dir" branch --show-current)"
git -C "$merge_message_dir" switch --quiet -c side-work
printf '%s\n' "a harmless side change" >"$merge_message_dir/docs/side.md"
git -C "$merge_message_dir" add docs/side.md
git -C "$merge_message_dir" commit --quiet -m "Finish the side work"
git -C "$merge_message_dir" switch --quiet "$primary_branch"
printf '%s\n' "a harmless main-line change" >>"$merge_message_dir/src/main.rs"
git -C "$merge_message_dir" add src/main.rs
git -C "$merge_message_dir" commit --quiet -m "Prepare the main line"
git -C "$merge_message_dir" merge --quiet --no-ff side-work \
  -m "Merge work from /home/${PERSONAL_ACCOUNT}/gmm"
expect "a leak in a merge commit subject is caught" 1 \
  "$merge_message_base" "$(git -C "$merge_message_dir" rev-parse HEAD)" \
  "$merge_message_dir"
rm -rf "$merge_message_dir"

# Message coverage above does not prove the diff spans the whole range: those
# fixtures leak only through commit messages, and the multi-file fixture is a
# single commit. Between them a checker diffing only the tip commit stays
# green. Here the added-line leak is in an earlier commit behind a clean one.
buried_added_dir="$(mktemp -d)"
setup_repo "$buried_added_dir"
buried_added_base="$(git -C "$buried_added_dir" rev-parse HEAD)"
printf '%s\n' "leak: /home/${PERSONAL_ACCOUNT}/gmm" \
  >>"$buried_added_dir/src/main.rs"
git -C "$buried_added_dir" add src/main.rs
git -C "$buried_added_dir" commit --quiet -m "Change the implementation"
printf '%s\n' "a harmless line" >>"$buried_added_dir/docs/notes.md"
git -C "$buried_added_dir" add docs/notes.md
git -C "$buried_added_dir" commit --quiet -m "Update the notes"
expect "an added-line leak in an earlier commit is caught" 1 \
  "$buried_added_base" "$(git -C "$buried_added_dir" rev-parse HEAD)" \
  "$buried_added_dir"
rm -rf "$buried_added_dir"

# Every case above puts its leak among the first added lines of a single
# changed file, so a checker examining only the head of the diff would pass
# them all. Here the commit touches several files and the leak is in the last
# one, well past the first added lines.
buried_diff_dir="$(mktemp -d)"
setup_repo "$buried_diff_dir"
buried_diff_base="$(git -C "$buried_diff_dir" rev-parse HEAD)"
for filler in src/a.rs src/b.rs docs/c.md docs/d.md; do
  mkdir -p "$buried_diff_dir/$(dirname "$filler")"
  printf 'harmless one\nharmless two\nharmless three\nharmless four\n' \
    >"$buried_diff_dir/$filler"
done
printf 'harmless\nharmless\nharmless\nleak: /home/%s/gmm\n' "$PERSONAL_ACCOUNT" \
  >"$buried_diff_dir/src-tauri/src/core/last.rs"
git -C "$buried_diff_dir" add -A
git -C "$buried_diff_dir" commit --quiet -m "Touch several files"
expect "a leak in a later file of a multi-file commit is caught" 1 \
  "$buried_diff_base" "$(git -C "$buried_diff_dir" rev-parse HEAD)" "$buried_diff_dir"
rm -rf "$buried_diff_dir"

# Added and modified files do not cover renamed files. This keeps enough of the
# original file for Git to classify the change as a rename, then adds the leak
# under its new name in the same commit.
renamed_dir="$(mktemp -d)"
setup_repo "$renamed_dir"
mkdir -p "$renamed_dir/docs/archive"
printf 'one\ntwo\nthree\nfour\nfive\nsix\n' \
  >"$renamed_dir/docs/archive/old-notes.md"
git -C "$renamed_dir" add docs/archive/old-notes.md
git -C "$renamed_dir" commit --quiet -m "Add notes to rename"
renamed_base="$(git -C "$renamed_dir" rev-parse HEAD)"
mkdir -p "$renamed_dir/docs/migrated"
git -C "$renamed_dir" mv docs/archive/old-notes.md docs/migrated/notes.md
printf '%s\n' "leak: /home/${PERSONAL_ACCOUNT}/gmm" \
  >>"$renamed_dir/docs/migrated/notes.md"
git -C "$renamed_dir" add docs/migrated/notes.md
git -C "$renamed_dir" commit --quiet -m "Rename the notes"
expect "a leak added while renaming a file is caught" 1 \
  "$renamed_base" "$(git -C "$renamed_dir" rev-parse HEAD)" "$renamed_dir"
rm -rf "$renamed_dir"

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

# Deliberately a sibling of the two exempt files: widening the exemption to
# .github/scripts/** must fail here, not merely a leak far away in the tree.
printf '%s\n' "leak: /home/${PERSONAL_ACCOUNT}/notes" \
  >"$scoped_dir/.github/scripts/unrelated-helper.sh"
git -C "$scoped_dir" add .github/scripts/unrelated-helper.sh
git -C "$scoped_dir" commit --quiet -m "and a leak elsewhere"
expect "a leak in a sibling guard-directory file is caught" 1 \
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
# abort rather than report a clean tree.
#
# The checker has two independent handlers -- one in match_all, one in
# filter_lines. A shim that breaks every grep proves only that *some* handler
# fired, so each would cover for the other and neither would be proven. These
# shims break exactly one call site each.
REAL_GREP="$(command -v grep)"

probe_broken_grep() {
  local name="$1" shim_body="$2"
  local dir output status
  dir="$(mktemp -d)"
  setup_repo "$dir"
  local base
  base="$(git -C "$dir" rev-parse HEAD)"
  printf '%s\n' "a harmless line" >>"$dir/src/main.rs"
  git -C "$dir" add src/main.rs
  git -C "$dir" commit --quiet -m "Fix the thing"
  mkdir -p "$dir/bin"
  {
    printf '%s\n' '#!/bin/sh'
    printf '%s\n' "$shim_body"
    printf 'exec %s "$@"\n' "$REAL_GREP"
  } >"$dir/bin/grep"
  chmod +x "$dir/bin/grep"
  set +e
  output="$(cd "$dir" && PATH="$dir/bin:$PATH" "$CHECKER" "$base" \
    "$(git -C "$dir" rev-parse HEAD)" 2>&1)"
  status=$?
  set -e
  if [ "$status" -ne 2 ]; then
    echo "FAIL: ${name}: expected exit 2, got ${status}"
    printf '%s\n' "$output" | sed 's/^/    /'
    failures=$((failures + 1))
  else
    echo "ok: ${name}"
  fi
  rm -rf "$dir"
}

# Only the scanning call uses -oEi.
probe_broken_grep "a broken scanning grep aborts" \
  'case "$*" in *-oEi*) exit 2 ;; esac'

# Only the added-line filters match on a leading plus.
probe_broken_grep "a broken added-line grep aborts" \
  'case "$*" in *"^+"*) exit 2 ;; esac'

if [ "$failures" -gt 0 ]; then
  echo "${failures} self-test case(s) failed"
  exit 1
fi

echo "personal-data guard self-tests passed"
