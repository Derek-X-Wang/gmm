# `recommended-importers.json`

GMM's curated answer to **which Model Importer package each game should
use** — layer 2 of the Importer Origin precedence in
[ADR 0005](../docs/adr/0005-importer-origin-conduit-not-maintainer.md).

Precedence, highest first:

1. the user's per-game override
2. **this file**
3. GMM's compiled-in defaults

GMM never maintains, forks, hosts or mirrors a Model Importer package.
This file only ever *points* at packages other people publish.

## Why editing this file is high-stakes

Every released build fetches this file at runtime from raw content on
`main`. There is no staged rollout and the CDN cache is measured in
minutes, so **a valid-but-wrong change reaches every install almost
immediately**. The review gate on `main` (`enforce_admins: true`) and
the offline shape validator are the only things in the way.

Before merging a change here, run:

```bash
cd src-tauri && cargo run --bin validate-manifest
```

It exits non-zero and names the offending game key or field. Point it at
another file with `-- <path>`.

The validator lives in its own workspace crate,
`src-tauri/crates/manifest-validator/`, rather than as a second binary
inside the Tauri package — a `src/bin/` entry there makes the bundler
ship the wrong executable.

## The rules

### The schema is additive only

Fields and status values may be **added**, never repurposed or
redefined. This is a discipline for whoever edits this file, not
something the client enforces.

It is what keeps already-shipped builds parsing this file successfully.
A build that meets a `schemaVersion` it does not recognise drops the
whole layer and tells the user their build is too old — so redefining
anything turns a routine edit into that prompt for every user who has
not updated.

### `none` retracts. Absence falls through. They are not the same.

| Intent | How to write it | What GMM does |
|---|---|---|
| Recommend an origin | `"status": "recommended"` + `owner`, `repo`, `assetPattern` | Proposes it to the user, who accepts or declines |
| GMM has no recommendation | `"status": "none"` (+ a `reason`) | **Retracts** the compiled-in default — *no* origin is in effect until the user supplies one |
| Let the game use its compiled-in default | **remove the key entirely** | Falls through to the compiled-in default |

**To hand a game back to its compiled-in default, delete its key. Do
not set it to `none`.** A `none` entry actively withdraws the default
and leaves the user with nothing until they choose an origin themselves.

That is intentional: you publish `none` precisely *because* the
compiled-in default has gone bad, so falling through would leave GMM
quietly recommending the exact thing it just declined to recommend.

The status is a tagged value rather than a `null` for the same reason —
`null` and a missing key are one keystroke apart, and many JSON tools
drop null-valued keys, which would silently turn a retraction into a
fall-through.

An unusable manifest — malformed JSON, an unknown `schemaVersion`, an
unrecognised status — is **never partially applied**. The whole layer
drops out and precedence collapses to user override → compiled-in
default.

## Shape

```json
{
  "schemaVersion": 1,
  "games": {
    "<game code>": { ... }
  }
}
```

Game codes are GMM's own: `gimi`, `srmi`, `zzmi`, `wwmi`, `himi`,
`efmi`. The validator rejects any key it does not recognise, because a
typo would otherwise sit here doing nothing while the game it was meant
for silently kept its old default.

### A `recommended` entry

| Field | Required | Notes |
|---|---|---|
| `status` | yes | `"recommended"` |
| `owner` | yes | GitHub owner. Compared case-insensitively |
| `repo` | yes | GitHub repository. Compared case-insensitively |
| `assetPattern` | yes | Release-asset match (see below) |
| `reason` | no | Short human-readable explanation shown in the prompt |

Only **GitHub origins** are recommended. A direct zip URL is not
allowed: it carries no release metadata, which blinds both the Importer
Pin and the update badge. A user may point a game at a local zip
themselves, but GMM never recommends one.

### A `none` entry

| Field | Required | Notes |
|---|---|---|
| `status` | yes | `"none"` |
| `reason` | no, but write one | Tell the user what to do — they are being asked to act |

### `assetPattern`

An **anchored regular expression** that must match **exactly one** asset
in the release. Anchoring is applied by GMM, not by you: write
`GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip`, not `^…$`.

Zero matches and two-or-more matches are both errors, and distinct ones.
This is the single asset-matching rule GMM has (#79); there is no
second one. A bare substring is what this replaced — `SRMI` matched
`SRMI-TEST-PACKAGE-v2.4.2.zip`, so GMM would have installed a build
upstream labelled TEST.

Remember JSON escaping: a regex `\d` is written `\\d` in this file.

## Current state

Five games are recommended and mirror GMM's compiled-in defaults.

**HIMI is retracted.** `leotorrez/HIMI-Package` last released
2025-07-24 and carries **no licence at all**, so there is no maintained
package GMM can recommend and none it could legally fork or mirror. Its
entry tells the user to supply their own origin.

## What does *not* live here

- Any check that a recommended origin still resolves. That is a
  scheduled job — the maintainer's process, not a pull request's, and
  not the user's screen. GMM never asks a user to judge whether an
  importer is abandoned, because age is a bad proxy for health.
- Signature verification. GMM verifies nothing today; pointing a game at
  a different origin does not change that.
