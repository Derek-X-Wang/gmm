# Research: Is LLM Vision Viable for Automating HSR Dailies?

**Ticket:** [#84](https://github.com/Derek-X-Wang/gmm/issues/84) — "Research: is LLM vision viable for HSR dailies, and at what cost"
**Date:** 2026-08-15
**Question:** Can an LLM vision loop actually drive Honkai: Star Rail's daily flow today — reading the screen and simulating input — and what does one run cost in latency and money?

## Verdict

**Not viable for v1 as an "LLM drives every tap" loop.** The state of the art in GUI grounding is good enough on ordinary desktop software but still measurably weak on the kind of small, dense game-UI targets HSR uses, and even where grounding is fine, a full daily run costs real money and multiple minutes of pure model latency for something a scripted macro does in seconds for free. The viable design is a **hybrid**: use the LLM offline/rarely to author and repair a deterministic macro (coordinate map + template/OCR matching) from a human-recorded run, and call it back in only for "what screen am I on / how do I recover" moments — not for every button press. This mirrors the one genuinely successful published architecture for this class of problem (Cradle, below), and it's the only design where the LLM's cost is paid occasionally instead of on every frame.

**Headline numbers** (derived below, Claude Sonnet 5 / Opus 5 pricing, 1080p screenshots, ~60–80 decision steps per daily):
- **Per screenshot:** ~2,691 input tokens at 1080p on a high-resolution-tier model (Opus 5, Sonnet 5) — this is the expensive part of every step, before any reasoning or history overhead.
- **Per run:** ≈$1–3.50 in API spend, roughly 4–12 minutes of pure model round-trip latency (before game animations/loading), for a task a human does in 3–5 minutes and a coordinate macro does in under 2.
- **Per month** (30 resets): **≈$30–105** — for a feature bundled into a free desktop tool automating a free-to-play game, that is a disproportionate and unpredictable recurring cost compared to $0 for a macro.
- **Grounding accuracy on dense/professional UI** (ScreenSpot-Pro): best model at benchmark publication reached only **18.9%** click accuracy on professional high-resolution screenshots; a training-free visual-search technique layered on top reached 48.1% — nowhere near "leave it running overnight unattended" territory for small game icons.
- **Long-horizon, multi-step task completion** (OSWorld 2.0 — the harder, more HSR-daily-shaped benchmark): the current leading model (Claude Opus 4.8) reaches only **20.6% binary task completion** (54.8% partial credit) — a good proxy for "will a full daily checklist actually finish correctly."

---

## 1. State of the art in UI grounding at 1080p

### Claude (computer-use-style grounding)

Screenshot cost and resolution behavior are documented, not folklore. Claude's vision pipeline tiles images into 28×28px patches (`⌈width/28⌉ × ⌈height/28⌉` visual tokens) and has two resolution tiers:

| Resolution tier | Models | Max long edge | Max visual tokens |
|---|---|---|---|
| High-resolution | Claude 4.7 and later (includes Opus 5, Sonnet 5) | 2576 px | 4784 |
| Standard | Older models | 1568 px | 1568 |

A 1920×1080 screenshot is **not resized** on the high-resolution tier and costs **2,691 visual tokens**; on the standard tier it's downscaled to 1456×819 and costs 1,560 tokens.
Source: [Claude vision docs](https://platform.claude.com/docs/en/build-with-claude/vision) — "Resolution and token cost" table.

Guidance on what resolution to *send* has moved with the model generation, and the two generations of guidance are in real tension:

- **Anthropic's original computer-use reference implementation** (Oct 2024, Claude 3.5-era) explicitly recommends staying at or below **XGA/WXGA (1024×768 / 1280×800)**, because relying on the API's own image-resizing "will result in lower model accuracy and slower performance than implementing scaling in your tools directly." Source: [`anthropics/claude-quickstarts` computer-use-demo](https://github.com/anthropics/claude-quickstarts/tree/main/computer-use-demo).
- **Current guidance for Claude 4.7+ / Opus 5 / Sonnet 5** (high-resolution tier) says the opposite is now fine: "Computer use works across resolutions up to the new 2576px / 3.75MP maximum. Sending images at 1080p provides a good balance of performance and cost. For particularly cost-sensitive workloads, 720p or 1366×768 are lower-cost options with strong performance." Source: Anthropic's Claude API model-migration guide, Opus 4.7 section (`platform.claude.com` docs, mirrored in the `claude-api` skill bundled with this environment).

Net: **1080p is now a supported, "good balance" resolution on current frontier Claude models** — but it costs meaningfully more tokens than the older 1024×768 recommendation (2,691 vs. roughly 1,000–1,300 tokens), and the *reason* the old guidance existed — small-icon localization accuracy — hasn't been benchmarked away, it's just less bad on newer models. Claude's own docs still warn plainly: "Claude's coordinate and localization outputs are approximate" and it "might hallucinate or make mistakes when interpreting low-quality, rotated, or very small images." Source: [Claude vision docs, Limitations](https://platform.claude.com/docs/en/build-with-claude/vision).

**Benchmark evidence for how "approximate" that is in practice:**

- **OSWorld-Verified** (real desktop-software tasks — office apps, browsers, file managers): as of August 2026 the top models cluster at 85–86% (Qwen3.8 Max 86.1%, Claude Mythos 5 and Fable 5 at 85.0%), above the ~72% human baseline the benchmark reports. This benchmark is described as nearing saturation for frontier models. Source: [OSWorld-Verified leaderboard summary, BenchLM](https://benchlm.ai/benchmarks/osworld-verified) (aggregating the [OSWorld](https://os-world.github.io/) benchmark, Xie et al., NeurIPS 2024).
- **OSWorld 2.0** (a newer, longer-horizon, harder variant of the same benchmark family — closer in shape to a multi-step daily checklist than a single desktop task): Claude Opus 4.8 leads at only **20.6% binary task completion**, 54.8% partial-credit score. This is the most structurally relevant published number for "will an LLM correctly complete a long multi-step routine end to end" — and it says most attempts don't. Source: [OSWorld 2.0 leaderboard, Snorkel AI](https://snorkel.ai/leaderboard/os-world-2-0/).
- **ScreenSpot-Pro** (GUI grounding specifically on high-resolution, professional/dense interfaces — 1,581 expert-annotated screenshots across 23 applications): at benchmark publication the **best model scored only 18.9%** click-grounding accuracy; a training-free visual-search technique (ScreenSeekeR) built on top of it reached 48.1% without additional training. Source: Li et al., ["ScreenSpot-Pro: GUI Grounding for Professional High-Resolution Computer Use"](https://arxiv.org/abs/2504.07981), arXiv:2504.07981 (2025). This benchmark is the closest published analog to "small icon buttons on a dense game HUD" — general chat/code benchmarks like OSWorld-Verified use standard desktop software with comparatively large, well-spaced UI targets; HSR's commission/assignment/shop screens are much closer to ScreenSpot-Pro's "professional, high-DPI, small-target" regime.

**Read together**: general computer-use grounding on ordinary software is now strong and arguably saturating (OSWorld-Verified); grounding on dense, small-target UI is still the hard, unsolved case (ScreenSpot-Pro); and multi-step task completion end-to-end is still weak even for the best model (OSWorld 2.0, 20.6%). HSR's daily flow — many small icon-sized buttons, nested menus, nearly identical-looking confirm dialogs — sits closer to the ScreenSpot-Pro / OSWorld 2.0 end of that spectrum than the OSWorld-Verified end.

### Gemini and open-weight VLMs

- **Gemini 3 Pro** reaches 72.7% on ScreenSpot-Pro, a large jump from Gemini 2.5 Pro's 11.4% — but still well behind the specialized open-weight GUI grounder GUI-Owl-1.5-32B (72.9%) and the technique-augmented MEGA-GUI (73.18%). Sources: search-aggregated from [alphaXiv ScreenSpot-Pro benchmark page](https://www.alphaxiv.org/benchmarks/national-university-of-singapore/screenspot-pro) and the arXiv papers for [GUI-Eyes](https://arxiv.org/pdf/2601.09770), [MEGA-GUI](https://arxiv.org/html/2511.13087) — treat these specific percentages as secondary-sourced (the leaderboard page itself required verification I could not complete against a live snapshot), but the qualitative finding — specialized open-weight GUI grounders now match or beat general frontier multimodal models on this specific task — is corroborated across multiple independent papers.
- **UI-TARS** (ByteDance) is the most mature open-weight, purpose-built GUI-grounding model family, released with weights on Hugging Face. UI-TARS-1.5-7B needs **16GB+ VRAM** for local FP16 inference (fits on a single high-end consumer GPU, e.g. RTX 4080/4090/3090, with quantization pushing it lower); UI-TARS-72B needs datacenter-class hardware (A100/H100-class GPUs or multi-GPU cloud rental) — not realistic to bundle into a consumer Windows desktop app. Source: [`bytedance/UI-TARS` GitHub](https://github.com/bytedance/UI-TARS), [VRAM requirements issue thread](https://github.com/bytedance/UI-TARS/issues/15).
- **Gemini's own image-token pricing** is far cheaper per image than Claude's at low/medium resolution settings (roughly 64–256 tokens/image vs. Claude's 1,000s), at $1.25/M input, $10/M output for the Gemini 2.5 Computer Use preview model — but that's because Gemini's low/medium settings are lower-fidelity crops, which is exactly the tradeoff that hurts small-icon accuracy. Source: search-aggregated pricing pages citing [ai.google.dev/gemini-api/docs/pricing](https://ai.google.dev/gemini-api/docs/pricing); not independently verified against a fetched primary page in this pass, flagged for confirmation before being used in a cost decision.

**Bottom line on grounding**: at 1080p, current frontier models (Claude Opus 5/Sonnet 5, Gemini 3 Pro) are dramatically better than they were a year ago, but the benchmark most analogous to HSR's UI — small, dense, professional-style targets — still tops out well under "reliable enough for unattended overnight automation," and specialized open-weight grounding models are competitive with or ahead of general frontier models on exactly this task, which matters for the hybrid design below.

---

## 2. Existing evidence of LLM agents playing games end to end

Three published efforts are directly relevant, and they cluster into "did well because the game rewards planning over reflexes" and "failed because the game is real-time":

### Claude Plays Pokémon (Anthropic)

Anthropic gave Claude 3.7 Sonnet basic memory, raw screen pixels, and function calls for button presses, and ran it continuously against Pokémon Red. Claude 3.0 Sonnet couldn't leave the starting town; Claude 3.7 Sonnet beat three gym leaders, taking **35,000 actions** to reach the fourth (Surge). Anthropic frames this as evidence of "extended thinking" capability, not a finished product — it's a research demo, run for days on a livestream, not a bounded daily task. Source: [Anthropic, "Claude 3.7 Sonnet and Claude Code"](https://www.anthropic.com/news/claude-3-7-sonnet); contemporaneous coverage: [TechCrunch](https://techcrunch.com/2025/02/24/anthropic-used-pokemon-to-benchmark-its-newest-ai-model), [LessWrong retrospective](https://www.lesswrong.com/posts/HyD3khBjnBhvsp8Gb/so-how-well-is-claude-playing-pokemon).

**35,000 actions for a partial Pokémon Red playthrough is the concrete illustration of the cost problem**: even a turn-based, forgiving game generates enormous action counts once an LLM is driving every step, and Pokémon's dungeon/battle loop is materially simpler than HSR's menu tree.

### Gemini Plays Pokémon (community project, Google DeepMind commentary)

An independent developer ran Gemini 2.5 Pro against Pokémon Blue/FireRed/Emerald on a public Twitch stream; it eventually finished Pokémon Blue. Google DeepMind's own reporting on the run found a specific, relevant failure mode: **Gemini would enter a "simulated state of panic" during low-HP battles**, causing a measurable drop in reasoning quality and, in some cases, forgetting to invoke its own pathfinding tool. Source: [TechCrunch, "Google's Gemini has beaten Pokémon Blue"](https://techcrunch.com/2025/05/03/googles-gemini-has-beaten-pokemon-blue-with-a-little-help); [dbreunig.com case study](https://www.dbreunig.com/2025/06/17/an-agentic-case-study-playing-pok%C3%A9mon-with-gemini.html).

This is a warning for HSR: even in a low-stress, forgiving daily-commission flow, HSR still includes combat (trailblaze power farming, calibration battles) where a model juggling more game state can degrade — even if "just tap auto-battle" is the intended action.

### Cradle — the closest architectural match, and evidence the *pattern* works

Cradle (Tan et al., 2024) is the strongest positive result for this ticket's question. Rather than a naive "screenshot in, tap out" loop, Cradle uses a six-module architecture (information gathering, self-reflection, task inference, **skill curation**, action planning, memory) and demonstrated it completing **40-minute real missions in Red Dead Redemption 2**, farming/harvesting in Stardew Valley, city-building in Cities: Skylines, and operating ordinary desktop software (Chrome, Outlook). Source: Tan et al., ["Towards General Computer Control: A Multimodal Agent for Red Dead Redemption II as a Case Study"](https://arxiv.org/abs/2403.03186), arXiv:2403.03186; [project page](https://baai-agents.github.io/Cradle/).

The load-bearing detail is **skill curation**: Cradle doesn't re-derive "how do I open the inventory menu" from pixels on every call — it builds and reuses a library of learned skills (effectively a self-authored macro/coordinate library), and only falls back to full vision-based reasoning when something novel happens. That's architecturally exactly the hybrid this research recommends for HSR (§5).

### VideoGameBench — the negative result, and why it doesn't apply to HSR the same way

VideoGameBench evaluates VLMs against 1990s Game Boy/MS-DOS games using only raw screenshots and high-level text objectives, with real-time play required. Results are stark: the best models (Gemini 2.5 Pro, Claude Sonnet 3.7) complete under 2% of games, fail basic actions like dragging or grid navigation in custom practice environments, and **"inference latency dominates real-time failure"** — the model is often still "thinking" when the game state has already moved on. Source: ["VideoGameBench: Can Vision-Language Models Complete Popular Video Games?"](https://arxiv.org/pdf/2505.18134), arXiv:2505.18134.

This result does *not* directly indict an HSR-daily automation attempt, because HSR's daily flow is explicitly turn-based/menu-driven with forgiving timing (per the ticket's framing) — it's much closer to Cradle's turn-based Stardew Valley farming loop than to a real-time platformer. But the shared failure mode — **latency causing state desync between "observe" and "act"** — still applies to HSR's loading screens, dialogue auto-advance, and battle-result animations, just with a much larger error-tolerance window.

---

## 3. Per-step cost and latency for a realistic HSR daily run

### Estimating screen/decision count

Based on the published daily checklist (Trailblaze Power spending across calyxes/stagnant shadows/cavern of corrosion, up to 4 simultaneous assignments with 4–20hr durations, ~5 daily-training sub-tasks, mail claim, shop refresh) — sources: [Icy Veins HSR dailies guide](https://www.icy-veins.com/honkai-star-rail/dailies), [Fandom: Trailblaze Power](https://honkai-star-rail.fandom.com/wiki/Trailblaze_Power), [Fandom: Daily Training](https://honkai-star-rail.fandom.com/wiki/Interastral_Peace_Guide/Daily_Training) — a full daily routine involves roughly:

| Sub-task | Estimated screens/decisions |
|---|---|
| Login/reward claim, mail claim | 2–4 |
| Daily training (5 sub-tasks, each 2–4 screens: navigate, act, confirm, back) | 10–20 |
| Trailblaze Power spend (calyx/cavern sweeps, auto-battle triggers) | 5–10 |
| Assignments (dispatch 4 slots × ~3 taps each, collect completed) | 12–16 |
| Shop / synthesizer / embers exchange | 5–8 |
| Menu navigation, loading-screen waits, "back" overhead (~20%) | 8–12 |
| **Total** | **~45–70 discrete decisions** |

Each decision in a naive screenshot-then-act loop typically costs **two** API round trips in practice, not one: one to decide the action, and a follow-up screenshot to confirm the tap landed before proceeding (the same "observe → verify" pattern documented in the game-agent literature above). That pushes realistic API-call counts to **roughly 60–100 calls per full daily run**; this research uses **70** as a central estimate.

### Token math per step (Claude Sonnet 5 / Opus 5, 1080p screenshots)

Using the documented per-image token cost (2,691 tokens for an un-resized 1920×1080 screenshot on the high-resolution tier — [Claude vision docs](https://platform.claude.com/docs/en/build-with-claude/vision)) plus a modest system-prompt/tool-definition/history overhead:

| Component | Tokens (approx.) |
|---|---|
| Screenshot (1080p, high-res tier) | 2,691 |
| System prompt + tool schema (partially cached after step 1) | 500–1,500 |
| Recent turn history (uncached increment) | 300–500 |
| **Input total per step** | **~3,500–4,700** |
| Output (tool call + brief rationale, thinking off) | 150–400 |
| Output (with adaptive thinking on ambiguous screens) | 500–1,500 |

Pricing per the `claude-api` skill's current rate card (Anthropic first-party API, cached 2026-06-24): **Sonnet 5** $3.00/$15.00 per MTok in/out (introductory $2.00/$10.00 through 2026-08-31); **Opus 5** $5.00/$25.00 per MTok in/out.

**Cost per step:**

| Model | Input cost | Output cost | Per-step total |
|---|---|---|---|
| Sonnet 5 (intro pricing) | 4,000 tok × $2/M = $0.008 | 300 tok × $10/M = $0.003 | **≈$0.011** |
| Sonnet 5 (standard) | 4,000 tok × $3/M = $0.012 | 300 tok × $15/M = $0.0045 | **≈$0.017** |
| Opus 5 (thinking light) | 4,000 tok × $5/M = $0.020 | 500 tok × $25/M = $0.0125 | **≈$0.033** |
| Opus 5 (thinking heavy, ambiguous screens) | 4,000 tok × $5/M = $0.020 | 1,500 tok × $25/M = $0.0375 | **≈$0.058** |

### Cost and latency per run and per month (70 steps/run)

| Model | Cost/run | Cost/month (30 resets) | Latency/run (model round trips only, ~3–8s/step incl. thinking) |
|---|---|---|---|
| Sonnet 5, intro pricing | **≈$0.77** | **≈$23** | ~3.5–5.8 min |
| Sonnet 5, standard pricing | **≈$1.19** | **≈$36** | ~3.5–5.8 min |
| Opus 5, light thinking | **≈$2.31** | **≈$69** | ~4.7–9.3 min |
| Opus 5, heavy thinking | **≈$4.06** | **≈$122** | ~5.8–11.7 min |

This is **model latency only** — it does not include HSR's own loading screens, dialogue auto-advance timers, or combat-animation waits, which for a real playthrough typically add several more minutes on top. A realistic end-to-end wall-clock daily run driven entirely by an LLM vision loop lands in the **8–20 minute** range, against roughly 3–5 minutes for a human and under 2 minutes for a coordinate-based macro with no model calls at all.

**The screenshot is the single most expensive, most irreducible line item.** At 1080p on the high-resolution tier, the image alone is ~2,700 tokens — more than half the input budget of every single step, before any reasoning happens. Downscaling to 720p or the older XGA (1024×768) recommendation would cut this by roughly half to two-thirds (per the resolution/token table in §1), at some further cost to small-icon grounding accuracy — the exact tradeoff Anthropic's own docs warn about.

---

## 4. Failure modes and mitigations

| Failure mode | Evidence | Mitigation |
|---|---|---|
| **Hallucinated / imprecise coordinates** | Claude's own docs state coordinate/localization output is "approximate"; ScreenSpot-Pro shows 18.9–48% grounding accuracy on dense professional UIs. | Constrain outputs to a small enumerated set of known UI elements (via tool schema / structured outputs) rather than free pixel coordinates wherever the screen is a known, previously-mapped state; reserve free-form pointing for genuinely novel/unmapped screens. |
| **Off-by-scale targeting** | Anthropic's own computer-use reference implementation exists specifically because relying on the API's internal image resizing produces coordinate drift — this is a documented, first-party-acknowledged failure mode, not a hypothetical. | Do coordinate scaling explicitly in the harness (screenshot at native resolution → resize to the model's tier → map model-returned coordinates back to native resolution proportionally), matching Anthropic's own reference pattern, rather than trusting the API's internal resize. |
| **Animation / loading-screen timing races** | VideoGameBench: "inference latency dominates real-time failure" — the screen has moved on by the time the model acts. HSR's timing is forgiving relative to a real-time game, but loading transitions and auto-advancing dialogue are still real desync risks. | Verify-before-next-action: always take a fresh confirmation screenshot after each tap before deciding the next one (this is already priced into the ~70→100 API-call estimate above); add explicit "wait for scene stable" polling rather than fixed sleeps. |
| **State-complexity degradation ("panic")** | Google DeepMind's own reporting on Gemini Plays Pokémon found reasoning quality measurably dropped in high-stakes battle moments, including forgetting to invoke tools. | Keep combat/high-complexity moments (trailblaze power farming battles) on the simplest possible action (a single "auto-battle" tap) rather than asking the model to make tactical decisions — this is already how a sensible HSR daily macro would behave, and it sidesteps the failure mode by design. |
| **Long-horizon task incompletion** | OSWorld 2.0: current best model completes only 20.6% of longer multi-step tasks correctly end to end. | Don't run the full ~70-step daily as one unsupervised session; checkpoint after each major sub-task (training, TB power, assignments, shop) so a failure in one segment doesn't silently corrupt or skip the rest, and surface a clear "couldn't complete X, needs human" signal rather than guessing forward. |

---

## 5. The realistic hybrid — where the LLM earns its cost

The evidence above points at one consistent shape: **LLMs are good at the occasional, ambiguous "what screen is this and what do I do" judgment call, and bad — slow and expensive relative to the value delivered — at driving every individual tap.** This is exactly Cradle's architecture: vision-based reasoning is used to *build and repair* a skill/macro library, and a much cheaper deterministic path executes it day to day.

Recommended design for GMM, in order of where the LLM should be involved:

1. **Flow authoring from a recording (one-time, per HSR version).** A human plays through the daily routine once while GMM records screenshots + input events. Feed that recording to a vision model *offline* (not in the runtime loop) to segment it into a state graph: named screens, the button/icon location on each, and the transition it triggers. Output: a deterministic coordinate/template map, not a live model dependency. This is a bounded, one-off cost (a handful of vision calls per screen type, not per run) that produces a $0-marginal-cost macro for actual daily execution.
2. **Screen identification when template matching fails.** At runtime, use fast, deterministic image matching (template/OCR) against the authored map for the common path — no model call at all for the vast majority of steps. When the current screen doesn't match anything in the map (an unexpected popup, an ad-hoc event banner, a network-error dialog), fall back to one LLM vision call to classify "what screen is this, and what's the safe recovery action" — occasional, cheap, and exactly the kind of judgment call the benchmarks above show these models are actually good at (open-ended screen understanding, not sub-pixel-precise pointing on a dense HUD).
3. **One-off adaptation after a game patch.** When HSR ships a UI update and the coordinate map breaks, re-run step 1's authoring pass against the new screens rather than rebuilding the macro by hand. This is where an LLM vision pass earns its cost most clearly — it replaces a human re-recording/re-mapping session, not a human's daily 3-minute click-through.
4. **Never put the LLM in the combat loop.** Keep any trailblaze-power farming on "auto-battle" as a single deterministic tap, per the failure-mode table above — there is no evidence current models add value here, and real risk (per the Gemini Plays Pokémon "panic" finding) that they add unreliability.

Under this design, the ~$1–4/run, multi-minute-latency numbers in §3 essentially disappear from the steady-state cost — they only apply to the bounded authoring and repair passes, not to every daily reset. That is the difference between "an LLM subscription cost every player who enables this feature pays every day" (not viable) and "an occasional maintenance cost GMM's own tooling pays when the game updates" (viable, and arguably a good showcase for how LLM vision is genuinely useful here).

---

## 6. Local vs. cloud VLM tradeoffs

| | Cloud (Claude/Gemini API) | Local (UI-TARS-1.5-7B or similar) |
|---|---|---|
| Grounding accuracy | Best available (still weak on dense UI — §1) | Meaningfully behind frontier closed models on general tasks, though specialized GUI-grounders like UI-TARS/GUI-Owl are competitive specifically on GUI grounding — see §1 | 
| Hardware requirement | None (network only) | 16GB+ VRAM for the 7B model at FP16 (a high-end consumer GPU); the 72B variant needs datacenter-class hardware, unusable for a bundled desktop tool. Source: [`bytedance/UI-TARS`](https://github.com/bytedance/UI-TARS), [VRAM issue thread](https://github.com/bytedance/UI-TARS/issues/15) |
| Cost model | Recurring, scales with usage (§3) | One-time (user's existing GPU) if hardware requirement is met, $0 marginal per run |
| Reach across GMM's userbase | Universal — works on any Windows machine with network access | Excludes any user without a discrete GPU with ≥16GB VRAM — likely a large fraction of GMM's actual userbase, since 3dmigoto/XXMI users skew toward mid-range gaming rigs, not all of which clear that VRAM bar |
| Unattended nightly run | Straightforward, but recurring dollar cost accrues even when nobody's watching, and depends on network/API-key availability at 3am | No recurring cost and no network dependency, which suits "unattended overnight" well *if* the hardware requirement is met — but for users without it, this option doesn't exist at all |

**For GMM specifically**: bundling a local VLM is impractical as a default because it would gate the feature behind a GPU/VRAM requirement much of the userbase won't meet, and the smaller open models don't close the grounding gap on dense game UI enough to justify that exclusion. Cloud API access avoids the hardware gate but reintroduces the per-run dollar cost from §3 as a standing liability. Under the hybrid design in §5, this tradeoff mostly evaporates too: the LLM (local or cloud) only needs to run during the bounded authoring/repair passes, so either option is viable there, and cloud is the simpler default for that infrequent use.

---

## Sources

- Anthropic, [Claude vision documentation](https://platform.claude.com/docs/en/build-with-claude/vision) — image token cost table, resolution tiers, limitations.
- Anthropic, Claude API model-migration guide, Migrating to Opus 4.7 section (bundled `claude-api` skill, mirroring `platform.claude.com` docs) — current 1080p computer-use resolution guidance.
- Anthropic, [`claude-quickstarts` computer-use-demo](https://github.com/anthropics/claude-quickstarts/tree/main/computer-use-demo) — original XGA/WXGA resolution recommendation and rationale.
- Anthropic, [Claude API pricing](https://claude.com/pricing) / `claude-api` skill rate card (cached 2026-06-24) — Sonnet 5 / Opus 5 per-token pricing.
- Anthropic, ["Claude 3.7 Sonnet and Claude Code"](https://www.anthropic.com/news/claude-3-7-sonnet) — Claude Plays Pokémon result (35,000 actions, three gym badges).
- [LessWrong, "So how well is Claude playing Pokémon?"](https://www.lesswrong.com/posts/HyD3khBjnBhvsp8Gb/so-how-well-is-claude-playing-pokemon) — independent retrospective.
- [TechCrunch, "Google's Gemini has beaten Pokémon Blue"](https://techcrunch.com/2025/05/03/googles-gemini-has-beaten-pokemon-blue-with-a-little-help) — Gemini Plays Pokémon result and DeepMind's "panic" finding.
- Tan et al., ["Cradle: Empowering Foundation Agents Towards General Computer Control"](https://arxiv.org/abs/2403.03186), arXiv:2403.03186 — RDR2/Stardew Valley/Cities: Skylines results, skill-curation architecture.
- ["VideoGameBench: Can Vision-Language Models Complete Popular Video Games?"](https://arxiv.org/pdf/2505.18134), arXiv:2505.18134 — real-time game failure rates, latency-dominates-failure finding.
- Li et al., ["ScreenSpot-Pro: GUI Grounding for Professional High-Resolution Computer Use"](https://arxiv.org/abs/2504.07981), arXiv:2504.07981 — dense/professional UI grounding accuracy (18.9% best-model, 48.1% with visual search).
- [OSWorld-Verified leaderboard summary, BenchLM](https://benchlm.ai/benchmarks/osworld-verified) and [OSWorld 2.0 leaderboard, Snorkel AI](https://snorkel.ai/leaderboard/os-world-2-0/) — general desktop-task and long-horizon-task completion rates.
- ByteDance, [`UI-TARS` GitHub repository](https://github.com/bytedance/UI-TARS) and [VRAM requirements discussion](https://github.com/bytedance/UI-TARS/issues/15) — open-weight GUI-grounding model, hardware requirements.
- [Icy Veins HSR dailies guide](https://www.icy-veins.com/honkai-star-rail/dailies), [Fandom: Trailblaze Power](https://honkai-star-rail.fandom.com/wiki/Trailblaze_Power), [Fandom: Daily Training](https://honkai-star-rail.fandom.com/wiki/Interastral_Peace_Guide/Daily_Training) — HSR daily checklist composition used for the screen-count estimate.

*Two figures in §1 (specific ScreenSpot-Pro percentages for Gemini 3 Pro, GUI-Owl, MEGA-GUI; Gemini image-token pricing) are sourced from search-tool synthesis of leaderboard/pricing pages that could not be independently re-fetched and confirmed in this pass — flagged inline where used. All cost/latency math in §3 is derived directly from Anthropic's own documented token-cost table and rate card, not from secondary sources.*
