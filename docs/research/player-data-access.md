# Research: can GMM give an AI agent the player's own account data?

**Question:** Can GMM expose, read-only and as machine-readable data, what a player actually owns — gear, echoes, relics, artifacts, weapons, character levels and stats — so that a separate AI agent running GMM's CLI can judge what is good and what to farm next?
**Scope:** The six supported Games: GIMI (Genshin Impact), SRMI (Honkai: Star Rail), ZZMI (Zenless Zone Zero), WWMI (Wuthering Waves), HIMI (Honkai Impact 3rd), EFMI (Arknights: Endfield). Research only. No production code. Deciding what gear is "good" is the agent's job, not GMM's, and is not covered here.
**Researched:** 2026-10-13. No game account, cookie or token was used. Every credential route below is described from its source code or documentation, not from a live call.

**Markers used throughout:**

- **Verified** — I read the live response, the source file, or the primary document myself.
- **Reported** — I rely on someone else's account: a third-party library's source, an unofficial API reference, or a search-engine excerpt of a page I could not load. A reported claim is not stated as fact.

---

## Verdict

**Full inventory is not reachable for any of the six Games by any route GMM may use.** "Full inventory" means every piece of gear the player owns, including unequipped pieces, with substats. Every tool that produces one today does it in one of two ways. It drives the game client with simulated keyboard and mouse input and reads the screen with OCR (Inventory Kamera, HSR-Scanner). Or it captures the game's network packets (Irminsul, reliquary-archiver). Both cross this task's hard lines.

What *is* reachable, by Game:

| Game | Full inventory | Gear on every owned character | Gear on a few showcased characters | Lowest-risk route |
| --- | --- | --- | --- | --- |
| GIMI — Genshin Impact | No acceptable route | HoYoLAB Battle Chronicle — **needs login cookie** | Enka.Network — UID only | Enka.Network |
| SRMI — Honkai: Star Rail | No acceptable route | HoYoLAB Battle Chronicle — **needs login cookie** | Enka.Network — UID only | Enka.Network |
| ZZMI — Zenless Zone Zero | No acceptable route | HoYoLAB Battle Chronicle — **needs login cookie** | Enka.Network — UID only | Enka.Network |
| HIMI — Honkai Impact 3rd | No acceptable route | Names and rarity only, no stats — HoYoLAB, **needs login cookie** | None found | None worth building |
| WWMI — Wuthering Waves | No acceptable route | China server only, via the Kuro BBS app token (reported). **Global server: none found** | None found | **None** |
| EFMI — Arknights: Endfield | No acceptable route | SKPORT data card, **needs full account token**; ToS explicitly forbids it | None found | **None** |

In plain terms:

- **For Genshin, Star Rail and ZZZ, a useful, low-risk slice exists.** With only the player's public UID, Enka.Network returns the characters the player chose to show on their in-game profile, with full gear and substats. That is a handful of characters, not the account.
- **The same three Games can give equipped gear on every owned character**, through HoYoverse's own HoYoLAB web service. That requires the player's HoYoLAB login cookie, which is a real credential, and it is an undocumented API that HoYoverse's terms do not permit.
- **Wuthering Waves — the motivating example — has no acceptable route for a global-server player.** Honkai Impact 3rd and Endfield have nothing worth building either.

**Recommendation on "one feature or two":** build it, if at all, as a **separate read-only tool that ships alongside GMM**, not as commands inside `gmm-cli`. The reasons are in [§ One feature or two](#8-one-gmm-feature-or-two).

---

## How this differs from the parked #81 automation effort

[#81](https://github.com/Derek-X-Wang/gmm/issues/81) (parked) and its children [#86](https://github.com/Derek-X-Wang/gmm/issues/86)–[#90](https://github.com/Derek-X-Wang/gmm/issues/90) are about GMM *playing* Star Rail: capturing the client's window and injecting `SendInput` clicks to run dailies. This work does none of that. It never touches the running client: no screen capture, no simulated input, no gameplay. It only reads data the player can already see on a publisher website or a public profile mirror, and it hands that data to an agent that never acts in the game. The two efforts meet at exactly one point. The only tools that reach a *full* inventory today do it with #81's techniques: OCR plus simulated input to page through the inventory screen. This document declines that route. It does not reopen #81, and it proposes no emulator, no input injection and no gameplay automation.

---

## Coverage levels used in this document

| Level | Meaning |
| --- | --- |
| **Inventory** | Every owned piece of gear, equipped or not, with main stat and substats. |
| **Equipped, all characters** | Every owned character, each with the gear it currently wears. Unequipped gear is invisible. |
| **Showcase** | Only the characters the player put on their in-game profile showcase, with their equipped gear. |
| **Summary** | Account-level numbers only (level, stamina, counts). No gear. |
| **None** | Nothing reachable. |

A route at "Showcase" is not an inventory and must never be presented as one.

---

## Per-game findings (questions 1–5)

Columns: **Route** (Q1), **Reaches** (Q2), **Auth** (Q3), **Risk** (Q4), **Stability** (Q5).

### GIMI — Genshin Impact

| Route | Reaches | Auth | Risk | Stability | Marker |
| --- | --- | --- | --- | --- | --- |
| **Enka.Network** `GET https://enka.network/api/uid/<uid>/` | **Showcase.** `playerInfo` plus `avatarInfoList`: level, constellations, skills, combat stats, weapon (level, ascension, refinement) and each artifact's set, slot, level, main stat and substats with values. A live call returned 4 characters. No unequipped artifacts. | Public UID only. | Low. Third-party service reading the public in-game showcase. Enka asks callers not to enumerate UIDs, to set a custom `User-Agent`, and to cache for the returned `ttl`. Dynamic rate limits end in HTTP 429. | Documented by Enka, not by HoYoverse. Enka documents HTTP 424 as "Game maintenance / everything is broken after the game update". Names come back as IDs and text-map hashes; resolving them needs separate data files. | **Verified** — live response 2026-10-13; [Enka API docs](https://github.com/EnkaNetwork/API-docs/blob/master/api.md), [Genshin docs](https://github.com/EnkaNetwork/API-docs/blob/master/docs/gi/api.md) |
| **HoYoLAB Battle Chronicle** `.../game_record/genshin/api/character/list` then `character/detail` | **Equipped, all characters.** Every owned character; for each, weapon with main and sub stat, and each artifact with `main_property` and `sub_property_list`. Names come back localised. | **HoYoLAB login cookie** (`ltuid` + `ltoken`, now `_v2` variants). genshin.py also adds a "dynamic secret" (`DS`) header to every HoYoLAB request, computed from a salt it hard-codes. HoYoverse does not publish that salt. | Medium–high. See [Credentials](#credentials-read-this-before-any-cookie-or-token-route). HoYoverse puts Geetest CAPTCHA challenges on Battle Chronicle traffic. Undocumented API; HoYoverse terms forbid scraping the services. | Undocumented and has broken before: genshin.py's changelog records "Removed all_characters since the API no longer supports this feature", "Fix v2 cookie compatibility", and "Fixed breaking API changes caused by the second banner". | **Reported** — [genshin.py routes](https://github.com/thesadru/genshin.py/blob/master/genshin/client/routes.py), [character models](https://github.com/thesadru/genshin.py/blob/master/genshin/models/genshin/chronicle/characters.py), [errors](https://github.com/thesadru/genshin.py/blob/master/genshin/errors.py), [CHANGELOG](https://github.com/thesadru/genshin.py/blob/master/CHANGELOG.md) |
| **Wish history URL in the client's web cache** — `output_log.txt` under `%USERPROFILE%\AppData\LocalLow\miHoYo\Genshin Impact\` names the game data folder; `webCaches\<version>\Cache\Cache_Data\data_2` holds a URL with an `authkey` | **Pull history only.** Which characters and weapons were pulled. No artifacts, no levels. Not gear. | An `authkey` embedded in the URL; genshin.py says it lasts 24 hours and cannot write. | Low–medium. Reading a cache file is passive, but the file can be locked while the game runs (the script copies it first). | Has survived several years, but it is an implementation detail of the client. | **Reported** — [Get-WishUrl.ps1](https://github.com/jogerj/genshin-wish-url/blob/main/Get-WishUrl.ps1), [genshin.py authentication](https://github.com/thesadru/genshin.py/blob/master/docs/authentication.md) |
| **Inventory Kamera** (OCR scanner) | Inventory. | None. | **Rejected — input automation.** Its README: "The scanner uses keyboard and mouse input to automate scanning." | — | **Verified** — [README](https://github.com/Andrewthe13th/Inventory_Kamera) |
| **Irminsul** (packet capture) | Inventory. | None. Needs admin rights. | **Rejected — packet interception.** Its README: "Irminsul utilizes packet capture instead of the common optical character recognition (OCR)". | — | **Verified** — [README](https://github.com/konkers/irminsul) |
| Memory reading | — | — | **Rejected — memory reading.** Not investigated further. | — | — |

### SRMI — Honkai: Star Rail

| Route | Reaches | Auth | Risk | Stability | Marker |
| --- | --- | --- | --- | --- | --- |
| **Enka.Network** `GET https://enka.network/api/hsr/uid/<uid>` | **Showcase.** `detailInfo.avatarDetailList`: level, promotion, eidolon (`rank`), skill tree, light cone, and each relic's set, slot, level, main affix and sub-affixes with a per-substat count field (`cnt`, commonly read as the number of rolls), plus pre-computed values in `_flat.props`. A live call returned 8 characters. | Public UID only. | Low — same terms as Genshin above. | As Genshin. The HSR endpoint is not in Enka's English docs index; its URL comes from the `enka-py` library, which also falls back to a backup host for non-China UIDs. | **Verified** (response shape) — live response 2026-10-13. URL **reported** from [enka-py constants](https://github.com/seriaati/enka-py/blob/main/enka/constants/common.py) |
| **HoYoLAB Battle Chronicle** `.../game_record/hkrpg/api/avatar/info` | **Equipped, all characters**, with light cone, relics and planar ornaments, each with its properties. | **HoYoLAB login cookie**, as Genshin. | Medium–high, as Genshin. | Undocumented, as Genshin. | **Reported** — [genshin.py Star Rail models](https://github.com/thesadru/genshin.py/blob/master/genshin/models/starrail/chronicle/characters.py) |
| **Warp history URL** in the web cache | Pull history only. Not gear. | 24-hour `authkey`. | Low–medium. | As Genshin. | **Reported** — [genshin.py authentication](https://github.com/thesadru/genshin.py/blob/master/docs/authentication.md) |
| **HSR-Scanner** (OCR) | Inventory. | None. Run as admin. | **Rejected — input automation.** README: "run as administrator (required to simulate keyboard and mouse presses)". | — | **Verified** — [README](https://github.com/kel-z/HSR-Scanner) |
| **reliquary-archiver** (packet capture) | Inventory. | None. | **Rejected — packet interception.** README: "tool to create a relic export from network packets". | — | **Verified** — [README](https://github.com/IceDynamix/reliquary-archiver) |

### ZZMI — Zenless Zone Zero

| Route | Reaches | Auth | Risk | Stability | Marker |
| --- | --- | --- | --- | --- | --- |
| **Enka.Network** `GET https://enka.network/api/zzz/uid/<uid>` | **Showcase.** `PlayerInfo.ShowcaseDetail.AvatarList`: level, mindscape, skills, W-Engine, and each Drive Disc's slot, level, main and random properties. Values are raw and need Enka's documented formulas. A live call returned 6 agents. Enka notes that disc and W-Engine `Uid`s are stable, so repeated calls can accumulate "all equipment on the account that was seen in the showcase" — still only what was shown. | Public UID only. | Low. | As Genshin. Raw values plus formulas mean more GMM-side logic to break. | **Verified** — live response 2026-10-13; [ZZZ docs](https://github.com/EnkaNetwork/API-docs/blob/master/docs/zzz/api.md) |
| **HoYoLAB Battle Chronicle** `.../game_record_zzz/api/zzz/avatar/basic` and `avatar/info` | **Equipped, all characters**: every agent, W-Engine, discs with main and sub properties. | **HoYoLAB login cookie.** | Medium–high. | Undocumented. | **Reported** — [genshin.py ZZZ models](https://github.com/thesadru/genshin.py/blob/master/genshin/models/zzz/character.py) |
| **Signal search URL** in the web cache | Pull history only. | 24-hour `authkey`. | Low–medium. | — | **Reported** — [genshin.py authentication](https://github.com/thesadru/genshin.py/blob/master/docs/authentication.md) |
| **Zenless Optimizer screenshot scanner** | Inventory, *if* the player opens every disc by hand while the tool OCRs screenshots or a shared screen. English UI only. | None. | Crosses no hard line itself (no input automation), but it is screen reading — #81's substrate — and costs the player one manual step per disc. **Not recommended for GMM.** | OCR breaks on UI changes. | **Verified** — [scanner UI source](https://github.com/frzyc/genshin-optimizer/tree/master/libs/zzz/ui/src/Disc/DiscEditor) |

### HIMI — Honkai Impact 3rd

| Route | Reaches | Auth | Risk | Stability | Marker |
| --- | --- | --- | --- | --- | --- |
| **HoYoLAB Battle Chronicle** `.../game_record/honkai3rd/api/characters` | **Thin.** Every battlesuit with its level, and the equipped weapon and stigmata as ID, name and rarity only. No equipment level, no stats. | **HoYoLAB login cookie.** | Medium–high, for very little data. | Undocumented. | **Reported** — [genshin.py HI3 models](https://github.com/thesadru/genshin.py/blob/master/genshin/models/honkai/chronicle/battlesuits.py) |
| Showcase mirror (Enka or similar) | **None found.** Enka's docs list Genshin and ZZZ, and its live API serves Star Rail; nothing for HI3. | — | — | — | **Verified** for Enka's docs index |

**Finding:** nothing for HI3 is worth building.

### WWMI — Wuthering Waves

| Route | Reaches | Auth | Risk | Stability | Marker |
| --- | --- | --- | --- | --- | --- |
| **Kuro launcher SDK** (`sdkapi.kurogame-service.com`, `pc-launcher-sdk-api.kurogame.net`) — global server | **Summary only**: base account info, stamina, daily activity, battle pass. No characters, no echoes. | **Account email and password**, plus a slider CAPTCHA. The client embeds SDK keys copied from Kuro's own client. | **Rejected.** It needs the account password, it impersonates Kuro's launcher, and it returns nothing useful. | Undocumented. | **Reported** — [XutheringWavesUID `api_sdk.py`](https://github.com/Loping151/XutheringWavesUID/blob/main/XutheringWavesUID/utils/api/api_sdk.py), [`launcher_chain.py`](https://github.com/Loping151/XutheringWavesUID/blob/main/XutheringWavesUID/utils/api/launcher_chain.py) |
| **Kuro BBS (库街区) app API** `api.kurobbs.com/aki/roleBox/...` | **Equipped, all characters** (`roleData`, `getRoleDetail`): each resonator with weapon and equipped echoes with their properties. China server confirmed. Global server: **could not establish.** The code carries global server IDs, but its own login path sends global accounts to the launcher SDK above, and its README directs global players to screenshot recognition. | **Kuro BBS app login token.** | High. Credential, undocumented, region-bound. | Undocumented; the bot ships its own signing helpers and CAPTCHA handling. | **Reported** — [XutheringWavesUID `api.py`](https://github.com/Loping151/XutheringWavesUID/blob/main/XutheringWavesUID/utils/api/api.py), [`requests.py`](https://github.com/Loping151/XutheringWavesUID/blob/main/XutheringWavesUID/utils/api/requests.py), [README](https://github.com/Loping151/XutheringWavesUID) |
| **Convene history URL in `Client\Saved\Logs\Client.log`** | Pull history only. Not gear. | IDs in the URL (`player_id`, `record_id`). | **Rejected.** The WuWa Tracker import script credits a decoder written "after Kuro added XOR obfuscation to the Client.log file", offers to strip deny-read ACLs from that log, and offers to edit the game's `Engine.ini` to turn logging back on. Editing game files crosses a hard line. Decoding an obfuscation the publisher added defeats a deliberate publisher measure. And the prize is not gear. | Kuro has already changed it once. | **Verified** — [import.ps1](https://github.com/wuwatracker/wuwatracker/blob/747a48b1b994baa9c372a4fb933ea7588428bd4b/import.ps1) |
| **In-game showcase mirror** | **None found.** WUWAFLEX states that the game offers no UID-only route ("like an API or an in-game character showcase"). | — | — | — | **Reported** — search excerpt of [WUWAFLEX](https://wuwaflex.com/); its page did not render for me |
| **Discord Game Stats Widget** (official Kuro × Discord) | Summary stats on the player's Discord profile. No API offered to the player. | Discord link. | — | — | **Verified** — [Discord case study](https://discord.com/developer-case-studies/wuthering-waves) |
| **`Client\Saved\LocalStorage\LocalStorage.db`** (SQLite) | Game settings, such as the frame-rate value. **Could not establish** whether it holds any account data. | None. | Opening it while the game runs may contend with the game's own lock. | — | **Reported** — [WuWa-Uncapper](https://github.com/Adim2P/WuWa-Uncapper) |
| Screenshot OCR (ScoreEcho, wuwa-optimizer) | One character's panel per screenshot the player takes. | None. | Screen reading; not GMM's job. | — | **Reported** |

**Finding:** for a global-server Wuthering Waves player there is **no acceptable route** to gear data today. This is an honest gap, not a design problem.

### EFMI — Arknights: Endfield

| Route | Reaches | Auth | Risk | Stability | Marker |
| --- | --- | --- | --- | --- | --- |
| **SKPORT game data card** `GET zonai.skport.com/api/v1/game/endfield/card/detail` | Probably **equipped, all characters**: `base` (operator count, weapon count), `chars[]` with level, promotion, potential, skills, weapon (level, refine, breakthrough, gem), and gear slots `bodyEquip`, `armEquip`, `firstAccessory`, `secondAccessory`, `tacticalItem`. **Could not establish** what the gear-slot objects contain (stats or names only), or whether `chars[]` lists every owned operator. | **Gryphline account token** — from email + password, or from a SKPORT session — exchanged through an OAuth grant using the SKPORT app's code, then a `cred` and `salt`, then HMAC-signed requests. | **High.** Endfield's terms explicitly forbid "unauthorized collection of data from the Services". | Unofficial reference, no licence, signing scheme copied from the official app. | **Reported** — [skport-api-docs](https://github.com/AixLnyt/skport-api-docs/blob/main/README_EN.md). ToS clause **verified** — [Endfield Terms of Service](https://user.gryphline.com/en-us/protocol/plain/endfield/game/terms_of_service), "Last Updated: September 2, 2026", §4(a)(iii)(C) |
| **Headhunting record API** `ef-webview.gryphline.com/api/record/...` | Pull history only. | Token in a URL exported from the game client. | Medium. | Undocumented. | **Reported** — same reference |
| Showcase mirror | **None found.** | — | — | — | — |

**Finding:** one route exists, behind a full account token, against an explicit prohibition in the terms. **Not acceptable** unless the maintainer knowingly accepts a terms breach.

---

## Risk ranking across routes (question 4)

Lowest risk first. Each claim here carries the marker of its row in the per-game tables above.

| Rank | Route | Games | Why |
| --- | --- | --- | --- |
| 1 | **Enka.Network showcase**, UID only | GIMI, SRMI, ZZMI | No credential. Reads data the player has chosen to make public in-game. Documented API with clear rules (no enumeration, custom User-Agent, honour `ttl`). The publisher sees nothing from GMM. A plausible account risk: none found. |
| 2 | **Pull-history URL from a client cache or log** | GIMI, SRMI, ZZMI | Short-lived read-only token. **Not gear**, so it does not answer this task's question. Listed for completeness. |
| 3 | **HoYoLAB Battle Chronicle**, login cookie | GIMI, SRMI, ZZMI, HIMI | Real credential. Undocumented. Every request carries a dynamic-secret header built from a salt HoYoverse does not publish, so a client must copy what HoYoverse's own web client does. HoYoverse already rate-limits it (`-110` "Visits too frequently"), caps lookups of other accounts (`10101` "Cannot get data for more than 30 accounts per cookie per day") and challenges it with Geetest CAPTCHAs. Terms forbid scraping. **No evidence found either way** that HoYoverse has ever banned a game account for reading its own Battle Chronicle. |
| 4 | **Kuro BBS token** | WWMI (China server) | Credential, undocumented, signing helpers, region-bound. |
| 5 | **SKPORT account token** | EFMI | Full account token, often obtained from the account password. Terms name "unauthorized collection of data" explicitly. |
| Rejected | Kuro launcher SDK login | WWMI | Account password; impersonates the launcher; returns no gear. |
| Rejected | Decoding the obfuscated WuWa `Client.log`, or editing `Engine.ini` or ACLs to re-enable it | WWMI | Modifies game files; defeats a publisher measure. |
| Rejected | OCR scanners that simulate input | GIMI, SRMI | Input automation. |
| Rejected | Packet capture | GIMI, SRMI | Packet interception. |
| Rejected | Memory reading | All | Memory reading. |

**One risk specific to a mod manager.** Enka asks callers to identify themselves with a custom `User-Agent`. That is harmless, because Enka is not the publisher. But for any route that talks to the *publisher's* own servers, an honest `User-Agent` naming GMM would tell HoYoverse, Kuro or Gryphline that this account belongs to someone who runs a 3dmigoto mod manager. ADR 0004 exists because publishers have run ban waves keyed to mod usage. The only alternative is to imitate a browser, which this task's "nothing that hides from the client" rule rules out. This dilemma has no clean answer. It is one more reason to keep publisher-credential routes out of GMM.

**Publisher precedent.** HoYoverse's terms forbid content being "downloaded, scraped, ... or otherwise exploited without COGNOSPHERE express prior written permission", and forbid making "your Account information available to third parties" (**verified**, [HoYoPlay Terms of Service PDF](https://fastcdn.hoyoverse.com/static-resource-v2/2024/07/01/1ed4cce564ae5dee2f903d37e814b1e0_6707783454148192051.pdf)). HoYoverse sent a DMCA notice that took down the GenshinData datamining repository (**reported**, [Gamerbraves](https://www.gamerbraves.com/?p=133801)). Kuro's Fair Play Policy bans third-party tools including macros and data modification (**reported** — search excerpt of the [official policy](https://wutheringwaves.kurogames.com/en/main/news/detail/742); the terms page is JavaScript-rendered and did not load for me).

---

## Credentials: read this before any cookie or token route

**Only the Enka route needs no credential.** Every other gear route needs one, and each is more powerful than "read my gear":

| Credential | Route | What it can do beyond reading gear |
| --- | --- | --- |
| HoYoLAB `ltuid_v2` / `ltoken_v2` (plus `ltmid_v2`) | Battle Chronicle | Acts as the player on HoYoLAB. genshin.py's own docs warn: "The cookies used in these APIs are the same as the ones you use to log in to your hoyolab account and make payments." |
| HoYoLAB `cookie_token_v2` / `account_id_v2` | Code redemption, account actions | Wider than `ltoken`. **Never needed** for reading gear and must never be asked for. |
| Kuro BBS token | WuWa (China server) | Acts as the player on Kuro BBS. |
| Gryphline account token | Endfield | Account-level token; often minted from the account password. |
| Account password | Kuro launcher SDK, Gryphline email login | Everything. **GMM must never ask for an account password.** |

If the maintainer ever chooses a credential route, the design must answer these before any code:

1. **Where it is stored.** Windows Credential Manager (DPAPI-protected, per Windows user) — never the SQLite database, never a plain file, never an environment variable.
2. **Who can read it.** This is the uncomfortable part. DPAPI protects against other Windows users, **not against other programs running as the same user.** The consumer is an AI agent that runs commands as that same user. It can read anything GMM can read. GMM can refuse to *print* the credential, but it cannot stop a same-user process from fetching it. And whatever reaches the agent's context reaches a cloud model provider — exactly the "make your Account information available to third parties" that HoYoverse's terms forbid.
3. **How it gets in.** Through the GMM window, typed or pasted by the human. Never as a CLI argument: arguments show up in process lists and in agent transcripts. The agent must never be the one handling it.
4. **What happens on expiry.** Return a typed error (`credentialExpired`) with a remedy ("re-link in the GMM window"). Never re-login automatically, never store a password to make that possible.
5. **Least privilege.** Store only the narrowest cookie the route needs (`ltoken_v2`, not `cookie_token_v2`).
6. **Revocation.** One visible "forget" action per Game that deletes the stored credential.

---

## 6. CLI contract sketch

Goals: one command shape for every Game; per-game detail only where the Games genuinely differ; and a `coverage` block that makes it impossible to mistake a showcase for an inventory.

The examples use `gmm-account`, matching the recommendation in [§8](#8-one-gmm-feature-or-two). If the maintainer puts this inside `gmm-cli` instead, the same subcommands work as `gmm-cli account ...`. Output follows `docs/cli.md`: one JSON object on one line, `{"ok":true,"result":{...}}` or `{"ok":false,"error":{"kind":...,"message":...}}`, exit 0 / 1 / 2. Examples below are pretty-printed and use placeholder values.

### Commands

| Command | Required options | Result |
| --- | --- | --- |
| `sources` | Optional `--game CODE` | For each Game: available sources, the coverage each can deliver, whether it needs a credential, and whether one is linked (a boolean — never the credential). |
| `snapshot` | `--game CODE --source SOURCE`, plus `--uid UID` for UID-based sources | One Account Snapshot from one source. |

Game codes are GMM's existing ones: `gimi`, `srmi`, `zzmi`, `wwmi`, `himi`, `efmi`. Linking and forgetting a credential, if that ever exists, are window actions, not CLI commands.

Every command is read-only. There is no `--dry-run`, because there is nothing to write. Snapshots never take GMM's instance lock and never interact with a Game Session.

### `sources`

```json
{
  "ok": true,
  "result": {
    "games": [
      {
        "game": "srmi",
        "sources": [
          {
            "source": "enka-showcase",
            "coverage": { "roster": "partial", "equippedGear": "partial", "unequippedGear": "none" },
            "needs": "uid",
            "credentialLinked": null,
            "available": true
          }
        ]
      },
      {
        "game": "wwmi",
        "sources": [],
        "note": "No supported source for this Game. See docs/research/player-data-access.md."
      }
    ]
  }
}
```

### `snapshot` (illustrative SRMI result from the showcase source)

```json
{
  "ok": true,
  "result": {
    "schema": "gmm.account-snapshot/0",
    "game": "srmi",
    "source": {
      "id": "enka-showcase",
      "fetchedAt": "2026-10-13T12:00:00Z",
      "upstreamCachedUntil": "2026-10-13T12:01:00Z",
      "servedFromLocalCache": false
    },
    "player": { "uid": "<uid>", "level": 70 },
    "coverage": {
      "roster": "partial",
      "equippedGear": "partial",
      "unequippedGear": "none",
      "charactersReturned": 8,
      "charactersOwned": null,
      "explanation": "Only the characters the player placed in the in-game showcase. Characters not shown and all unequipped gear are unknown, not absent."
    },
    "characters": [
      {
        "id": "1001",
        "name": "<character name>",
        "level": 80,
        "ascension": 6,
        "rank": 0,
        "weapon": { "id": "21001", "name": "<light cone name>", "level": 80, "superimposition": 1 },
        "gear": [
          {
            "slot": "head",
            "set": { "id": "101", "name": "<relic set name>" },
            "rarity": 5,
            "level": 15,
            "main": { "key": "hp_flat", "value": 705.6 },
            "subs": [
              { "key": "crit_rate", "value": 0.0324, "rolls": 1 },
              { "key": "crit_dmg", "value": 0.1231, "rolls": 2 }
            ]
          }
        ],
        "gameSpecific": { "skillTree": [] }
      }
    ],
    "unequippedGear": null,
    "warnings": ["coverage.partial: showcase only"]
  }
}
```

### Shape rules

- **`null` means unknown; `[]` means known to be empty.** `unequippedGear: null` says "this source cannot see it", which is different from "the player has none". `charactersOwned: null` likewise.
- **`coverage` is mandatory and comes first in meaning.** Values: `complete`, `partial`, `none`. The agent must read it before drawing any conclusion.
- **Gear slots use a per-game vocabulary**, documented per Game: GIMI `flower|plume|sands|goblet|circlet`; SRMI `head|hands|body|feet|sphere|rope`; ZZMI `1`–`6`; HIMI `T|M|B`; WWMI by echo cost; EFMI `body|arm|accessory1|accessory2|tactical`.
- **Stat keys are per-game.** Keep a GMM key (`crit_rate`) only where the meaning is unambiguous, and include the upstream key (`sourceKey`) when the mapping is lossy. Percent stats are fractions (`0.0324`), not `3.24`.
- **`rolls`** appears only when the source gives it (Enka SRMI gives `cnt`). Otherwise the field is absent, not guessed.
- **Personal data is minimised.** No nickname, signature, birthday or friend count. The agent does not need them.
- **Errors are typed:** `uidRequired`, `playerNotFound` (Enka 404), `showcaseEmpty`, `rateLimited` (with `retryAfterSeconds`), `upstreamMaintenance` (Enka 424), `sourceUnavailable`, `unsupportedGame`, and — only if a credential route ever exists — `credentialMissing` and `credentialExpired`.

### What the contract must NOT promise

- **Completeness.** No source here gives a full inventory. The contract never implies one. No field may be filled with a guess.
- **Freshness.** Enka serves cached data until `ttl` runs out. The player may have re-geared since. Always report `fetchedAt` and the upstream cache time.
- **Comparability across Games.** `crit_rate` in two Games is not the same stat. The agent must not compare them blindly.
- **Stable IDs across sources.** An item ID from Enka and one from HoYoLAB are not guaranteed to match.
- **Judgement.** No "good gear" score, no rating, no meta, no recommendation, no build advice. That is the agent's job, informed by its own web research.
- **Writes of any kind.** No equip, lock, upgrade or check-in.

---

## 7. Smallest first slice

**Game: SRMI (Honkai: Star Rail). Route: Enka.Network showcase, UID only.**

Why this one:

- It needs **no credential**, so no credential design has to land first.
- Enka returns **pre-computed stat values** for Star Rail relics (`_flat.props`) and a per-substat count (`cnt`). That is less GMM-side formula work than ZZZ, whose disc values are raw.
- It is the route with the lowest risk on the ranking above.
- Genshin would also work. Wuthering Waves, the motivating example, cannot be the first slice, because it has no route.

A tracer-bullet implementation must show, end to end:

1. `gmm-account snapshot --game srmi --source enka-showcase --uid <uid>` returns the contract above, with `coverage.roster = "partial"` and `unequippedGear: null`.
2. Requests carry a custom `User-Agent` and respect Enka's `ttl` with a local cache. A second call inside the `ttl` is served from cache and says so (`servedFromLocalCache: true`).
3. Typed errors for 400, 404, 424 and 429. A closed or empty showcase gives `showcaseEmpty`, not an empty success.
4. Names resolved from data fetched at runtime — not vendored (see [§9](#9-licences-dependencies-and-service-terms)) — with IDs kept alongside names.
5. Tests run from a **recorded fixture** with the UID and nickname replaced by placeholders. No live network in CI.
6. A short demonstration: an agent given only the JSON answers "which equipped relics have both crit substats?" correctly, and refuses to claim anything about unequipped relics.

Out of the slice: any credential route, any Game other than SRMI, any background polling, any UI beyond what the tool needs.

---

## 8. One GMM feature or two?

**Recommendation: two.** Build it as a **separate read-only tool** — its own binary and its own data directory — that may live in this repository and may ship in the same installer, but that is **not** part of `gmm-cli` and stores nothing in GMM's database. The maintainer decides.

Reasons:

1. **It is a different job with a different risk.** GMM writes to game directories and is careful about one risk: ban waves keyed to mod usage (ADR 0004). This tool would make network reads of player accounts and might one day hold credentials. A bug or a breach in one should not touch the other.
2. **It breaks on a different schedule.** Showcase and Battle Chronicle shapes change with game patches. GMM updates conservatively. A separate tool can update on its own cadence without dragging a GMM release along.
3. **The mod manager's CLI has the wrong locking model.** `gmm-cli` takes GMM's instance lock and refuses to run while the window is open (`docs/cli.md`). An agent reading the player's gear has no reason to fight the mod manager or wait for a Game Session to end.
4. **The shared surface is tiny.** The two would share the six game codes and nothing else — not the Library, not Junctions, not the Loader, not Importer Origin.
5. **ADR 0005's logic applies.** ADR 0005 declined maintaining Model Importers because of the permanent ops burden of six packages rebroken by every patch. Maintaining reverse-engineered knowledge of six publishers' private APIs — dynamic-secret salts, signing schemes, app codes — is the same kind of burden, with credentials attached. A separate tool keeps that burden visibly separate and easy to drop.
6. **It keeps a mod manager's identity away from the publisher.** See the User-Agent dilemma in [§ Risk ranking](#risk-ranking-across-routes-question-4).
7. **Licensing stays clean.** Name-resolution data with unclear licences never enters GMM's GPLv3 distribution (see §9).

The case for one feature: a single install, one executable for the agent to call, one place to look. Shipping the separate binary in the same MSI, beside `gmm-cli.exe`, recovers most of that.

**A cheaper alternative worth weighing first.** The agent can already call Enka.Network itself — it is a public, documented HTTP API — and it can read screenshots the player gives it directly. If the only route GMM would ever ship is the Enka showcase, a short documented recipe for the agent may deliver most of the value with no new binary at all. A dedicated tool earns its place mainly by owning the per-game normalisation, the `coverage` contract and the caching rules, so that every agent gets the same honest answer.

### Domain terms this would need (proposed — `CONTEXT.md` is unchanged)

- **Account Snapshot** — one read of one player's account state, from one Account Source, at one time, with its Coverage.
- **Account Source** — where a snapshot comes from (for example the Enka showcase or the HoYoLAB Battle Chronicle). Distinct from **Source**, which is a Mod's origin.
- **Coverage** — what an Account Snapshot can and cannot see: roster, equipped gear, unequipped gear, each `complete`, `partial` or `none`.
- **Account Link** — a stored publisher credential for one Game, if a credential route is ever adopted. Created and removed only in the window.

---

## 9. Licences, dependencies and service terms

GMM is GPLv3 (ADR 0001). Nothing found needs to be linked; every useful library is Python or TypeScript and would serve as a reference only.

| Item | Licence or terms | Problem? | Marker |
| --- | --- | --- | --- |
| `genshin.py` (HoYoLAB client) | MIT | No. Reference only. | **Verified** — GitHub metadata |
| `enka-py` | GPL-3.0 | No. Reference only. | **Verified** |
| `enkanetwork-rs` (the only Rust Enka wrapper Enka lists) | **No licence**; last push 2024-01-09 | **Yes** — all rights reserved and stale. Do not use. | **Verified** |
| Enka `API-docs` repository, including `store/*.json` name tables | **No licence** | **Yes** for vendoring. Fetch at runtime and cache; do not bundle into the MSI. | **Verified** |
| Datamined game data (Dimbreath's repositories, which Enka's docs link) | Publisher content; GenshinData was taken down by DMCA | **Yes.** Never vendor. | **Reported** |
| `XutheringWavesUID` | GPL-3.0 | Licence is compatible, but it embeds Kuro client keys. Do not copy. | **Verified** licence |
| `skport-api-docs` | **No licence** | Read only; do not copy text or code. | **Verified** |
| WuWa Tracker `import.ps1` | GPL-3.0 | Licence fine; mechanism rejected. | **Verified** |
| Inventory Kamera (MIT), HSR-Scanner (GPL-3.0), Irminsul (MIT), reliquary-archiver (MIT) | Fine | Licences fine; every mechanism rejected. | **Verified** |
| Enka.Network API rules | No enumeration; custom `User-Agent`; honour `ttl`; 429 on abuse | Compatible with a per-player tool. Its `robots.txt` carries Cloudflare's "content signals" preamble but declares no signals, which by its own clause "neither grants nor restricts" AI use. | **Verified** |
| HoYoverse terms (HoYoPlay ToS) | Forbid scraping and making account information available to third parties | **Yes** for any HoYoLAB route. | **Verified** |
| Gryphline Endfield terms | §4(a)(iii)(C) forbids "unauthorized collection of data from the Services" | **Yes** for SKPORT. | **Verified** |
| Kuro terms and Fair Play Policy | Ban third-party tools, macros, data modification | **Yes** for any Kuro route. Exact text not read. | **Reported** |

Note that ToS exposure is not new for GMM's users: running 3dmigoto mods already conflicts with these terms' derivative-work clauses. What is new is linking a *credential* and a *publisher-side request pattern* to the account. That is why the Enka route is the only one recommended without reservation.

---

## What we could not establish

Do not read this document as complete. These gaps remain:

- **Whether HoYoLAB returns the player's own data when their Battle Chronicle privacy toggles are off.** genshin.py defines error `10102` "User's data is not public", but I could not test this without an account.
- **Whether HoYoverse has ever banned a game account for Battle Chronicle reads.** No evidence was found either way.
- **Whether a Kuro BBS token can read a global-server Wuthering Waves account.** The code is ambiguous (see the WWMI table).
- **Whether any Wuthering Waves local file other than the log** — for example `LocalStorage.db` — holds account data. No install was available to inspect.
- **What Endfield's SKPORT gear-slot objects contain**, and whether `chars[]` covers every owned operator.
- **Whether Endfield or Honkai Impact 3rd has any public profile showcase** that a mirror could read. None was found.
- **The exact text of Kuro's terms of service.** The page is JavaScript-rendered; I relied on search excerpts.
- **The maximum showcase size per Game.** The live calls returned 4 (Genshin), 8 (Star Rail) and 6 (ZZZ) characters for specific public sample accounts. These are observations, not documented limits.
- **Enka's Star Rail coverage of China-server UIDs.** enka-py falls back to a backup host only for non-China UIDs; I did not test China-server UIDs.
- **Whether the HoYoLAB endpoints named here still work today.** They are named from genshin.py's current source, not from live calls. No credential route was exercised, deliberately.
- **Where the player's UID would come from.** This document assumes the player supplies it. Whether a Game writes it to a readable local file was not investigated.

---

## Sources

**Verified** (read directly):

- Enka.Network API docs — [api.md](https://github.com/EnkaNetwork/API-docs/blob/master/api.md), [Genshin](https://github.com/EnkaNetwork/API-docs/blob/master/docs/gi/api.md), [ZZZ](https://github.com/EnkaNetwork/API-docs/blob/master/docs/zzz/api.md); live responses from `enka.network/api/uid/`, `/api/hsr/uid/` and `/api/zzz/uid/` on 2026-10-13, using sample UIDs from Enka's own docs and enka-py's tests (not reproduced here)
- [Enka.Network robots.txt](https://enka.network/robots.txt)
- [HoYoPlay Terms of Service (PDF)](https://fastcdn.hoyoverse.com/static-resource-v2/2024/07/01/1ed4cce564ae5dee2f903d37e814b1e0_6707783454148192051.pdf)
- [Arknights: Endfield Terms of Service](https://user.gryphline.com/en-us/protocol/plain/endfield/game/terms_of_service)
- [WuWa Tracker import.ps1](https://github.com/wuwatracker/wuwatracker/blob/747a48b1b994baa9c372a4fb933ea7588428bd4b/import.ps1)
- Scanner READMEs — [Inventory Kamera](https://github.com/Andrewthe13th/Inventory_Kamera), [Irminsul](https://github.com/konkers/irminsul), [HSR-Scanner](https://github.com/kel-z/HSR-Scanner), [reliquary-archiver](https://github.com/IceDynamix/reliquary-archiver)
- [Zenless Optimizer disc scanner UI](https://github.com/frzyc/genshin-optimizer/tree/master/libs/zzz/ui/src/Disc/DiscEditor)
- [Discord × Wuthering Waves case study](https://discord.com/developer-case-studies/wuthering-waves)
- Repository licence metadata for every library in §9, read through the GitHub API

**Reported** (relying on another party's account):

- [genshin.py](https://github.com/thesadru/genshin.py) — [routes](https://github.com/thesadru/genshin.py/blob/master/genshin/client/routes.py), [constants](https://github.com/thesadru/genshin.py/blob/master/genshin/constants.py), [errors](https://github.com/thesadru/genshin.py/blob/master/genshin/errors.py), [authentication docs](https://github.com/thesadru/genshin.py/blob/master/docs/authentication.md), [CHANGELOG](https://github.com/thesadru/genshin.py/blob/master/CHANGELOG.md), and the per-game chronicle models linked in the tables
- [enka-py constants](https://github.com/seriaati/enka-py/blob/main/enka/constants/common.py)
- [jogerj/genshin-wish-url](https://github.com/jogerj/genshin-wish-url/blob/main/Get-WishUrl.ps1)
- [XutheringWavesUID](https://github.com/Loping151/XutheringWavesUID)
- [skport-api-docs](https://github.com/AixLnyt/skport-api-docs/blob/main/README_EN.md)
- [WuWa-Uncapper](https://github.com/Adim2P/WuWa-Uncapper)
- [WuWa Tracker troubleshooting article](https://wuwatracker.com/en/articles/how-to-fix-wuthering-waves-pull-history-issues)
- Search excerpts of [WUWAFLEX](https://wuwaflex.com/) and [Kuro's Fair Play Policy](https://wutheringwaves.kurogames.com/en/main/news/detail/742)
- [Gamerbraves on the GenshinData DMCA takedown](https://www.gamerbraves.com/?p=133801)
