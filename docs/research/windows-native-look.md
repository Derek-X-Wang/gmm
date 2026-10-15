# Research: what would make the GMM window read as a Windows 11 application?

**Question:** #326 gave GMM a native menu, a real title bar, a bottom status bar and a modal
Settings dialog. All of it landed and was verified on Windows. The maintainer's verdict on the
`v0.4.2-alpha.1` screenshots was still *"the content is centered, which is really weird. It still
doesn't feel like a more native Windows app."* So: **what would actually make this window read as a
Windows 11 application, and what does each route cost?**

**Scope:** Appearance and layout of the existing surfaces. This feeds the decision in
[#365](https://github.com/Derek-X-Wang/gmm/issues/365), which frames four options (A, B, C, D) and
is assessed option by option below. Research only — this document changes no application code, CSS
or test.

**Researched:** 2026-10-14. No npm dependency was installed into the repository. Library facts come
from the npm registry, each project's own `LICENSE` file, and each project's own source.

**Markers used throughout:**

- **Verified** — I read the primary source myself: the Microsoft Learn page, the project's
  `LICENSE` file, the package's registry metadata, the library's source file, or this repository's
  own code and test run.
- **Reported** — I rely on someone else's account. A reported claim is not stated as fact.

Anything version-dependent carries its date.

---

## Verdict

**The centred column is the whole visible complaint, and it is a four-line CSS change.** Everything
else on the list — corner radius, button fills, type sizes, spacing rhythm, the font face — is also
CSS, in one file, with no new dependency and no markup change. Taken together that is **option A**,
and it is the only option that answers the complaint at all. Options B and C both decorate the
problem without fixing it.

| | What it fixes | Cost | New dependency |
| --- | --- | --- | --- |
| **A** Layout and density pass | The centred column, web control vocabulary, the font face, the type scale | 1 CSS file, ~1 test assertion | None |
| **B** Fluent component library | Control vocabulary, nothing else on the list | Every surface rewritten; 32 MiB installed; the Settings dialog's three load-bearing properties re-proved against someone else's component | `@fluentui/react-components`, MIT |
| **C** Mica / Acrylic only | Nothing on the list. Decorates a layout that is still centred, and requires a transparent window, which fights the native decorations #326 just gained | Rust-side, small, but risky | `window-vibrancy`, MIT/Apache-2.0 |
| **D** A then C | A's fixes, then a backdrop on a layout that deserves one | A's cost, then C's | C's |

**Recommended: A, as a first slice confined to `src/App.css`. Then re-look.** Not B — and the
reason is specific, not budgetary: **Fluent UI's own `fontFamilyBase` token is `'Segoe UI'`, not
Segoe UI Variable** (Verified, [source](https://github.com/microsoft/fluentui/blob/master/packages/tokens/src/global/fonts.ts)),
so adopting Fluent would not even fix #365's item 5. The one thing B buys that A cannot is a
*maintained* control vocabulary, and GMM has fifteen components, not a hundred.

The first slice is spelled out in [§6](#6-recommendation-and-a-first-slice). What it deliberately
does not do: no component library, no window material, no new dependency, no bundled font, no
two-pane layout, no change to the Settings dialog's geometry or containment.

---

## 1. What makes a web-technology window read as native on Windows 11

Everything in this section is checkable. Microsoft publishes the numbers.

### 1.1 Layout: fill the window, left-align, and pick a silhouette

Microsoft documents four **app silhouettes** and names a shipping Windows 11 app for each
(Verified, [Windows app silhouettes](https://learn.microsoft.com/en-us/windows/apps/design/basics/app-silhouette)):

| Silhouette | Example in Windows 11 | Content margins the page cites |
| --- | --- | --- |
| Top navigation | Photos | 56 epx — "complementing large pieces of media" |
| **Menu bar** | **Notepad** | **12 epx — "to compliment the utility of the app"** |
| Left navigation | Settings | 56 epx — "to compliment the cohesion of the content within the expanders" |
| Tab view | Terminal | 12 epx |

Two things follow for GMM.

First, **GMM is already a menu-bar silhouette.** #326 put a `File`/`Help` menu on the window and
pinned a status bar to the bottom. Microsoft's own example of that silhouette is Notepad, and the
margin it cites is **12 epx** — not 110px of empty ground on each side. The utility-app silhouette
uses small margins precisely because the content is the point.

Second, **not one silhouette centres a fixed-width column.** In all four, the content layer starts
at the left margin and runs to the right margin, whatever the window is. Microsoft's typography
guidance is explicit about alignment: *"The default TextAlignment is Left, and in most instances,
flush-left and ragged right provides consistent anchoring of the content and a uniform layout"*
(Verified, [Typography in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/typography)).
Fluent 2's web layout page says the same: *"align objects centrally and to align text left"*
(Verified, [Fluent 2 — Layout](https://fluent2.microsoft.design/layout)).

The one thing Microsoft does cap is **prose line length**, not window width: *"Keep to 50–60
letters per line for ease of reading"*, and *"Don't use fewer than 20 characters or more than 60
characters per line"* (Verified, same typography page). Fluent 2's layout page declines to give a
maximum content width at all and only recommends manuscript-style measures for large text blocks
(Verified). So the native pattern is: **the layout fills the window; individual paragraphs get a
measure.** A centred page is a reading-page convention, and that is exactly what the maintainer
saw.

Windows 11 also uses a **two-layer system** (Verified,
[Layering and elevation](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/layering)):

- the **base** layer, *"the bottommost layer of every app… contains controls related to app menus,
  commands, and navigation"*;
- the **content** layer, which *"focuses the user on the app's central experience… may be one
  contiguous element, or separated into cards that segment content."*

GMM's card-based main screen is already the card pattern of the content layer. What is missing is a
visible base layer under it — the menu is in the OS chrome and the status bar is a separate fixed
surface, so the two layers never read as layers.

### 1.2 Geometry: 8 / 4 / 0, and nothing else

Windows 11 uses exactly three corner radii (Verified,
[Geometry in Windows 11](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/geometry)):

| Radius | Usage, quoted |
| --- | --- |
| **8px** | *"Top-level containers such as app windows, flyouts and dialogs"* — ContentDialog, Flyout, MenuFlyout, TeachingTip |
| **4px** | *"In-page elements such as buttons and list backplates"* — Button, CheckBox, ComboBox, TextBox, ListView; also bar elements (ProgressBar, ScrollBar, Slider); also ToolTip, *"an exception, using 4px due to its small size"* |
| **0px** | *"Straight edges that intersect with other straight edges"*; snapped or maximized window corners; touching parts of one control, such as the two halves of a SplitButton |

The two global resources are named: `ControlCornerRadius` (default 4px) and `OverlayCornerRadius`
(default 8px) (Verified, same page). Fluent 2's web token ramp agrees and adds the rule for small
shapes: rectangles default to 4px, and *"for shapes smaller than 32 pixels, the corner angle is
reduced to 2 pixels"*; pills are reserved for slider tracks, toggle channels, tags and list
selections (Verified, [Fluent 2 — Shapes](https://fluent2.microsoft.design/shapes)). The
corresponding Fluent token values are `borderRadiusSmall: 2px`, `borderRadiusMedium: 4px`,
`borderRadiusXLarge: 8px`, `borderRadius2XLarge: 12px` (Verified,
[`borderRadius.ts`](https://github.com/microsoft/fluentui/blob/master/packages/tokens/src/global/borderRadius.ts)).

**Against that, `src/App.css` today** (Verified, read in this repository):

| Element | GMM now | Windows 11 says |
| --- | --- | --- |
| `.card` | `border-radius: 12px` | 8px — it is a card on the content layer, not a popover |
| `button` | `border-radius: 8px` | 4px |
| `.mods__row`, `.onboarding__*-row` | `border-radius: 8px` | 4px — these are list backplates |
| `input.path`, `input[type="text"]` | `border-radius: 6px` | 4px |
| `.game-tabs__tab` | `border-radius: 999px` | 4px — a tab bar is `None`/4px, never a pill |
| `.conflict-pill`, `.update-pill` | `border-radius: 999px` | 2px (both are under 32px tall) — pills are for sliders and toggles |
| `.settings-dialog` | `border-radius: 12px` | 8px — it is a dialog |
| `.dropzone` | `border-radius: 10px` | 4px or 8px, nothing in between |

Seven different radii where Windows 11 uses three. That alone reads as a web page.

Elevation is also specified as discrete values with a 1px stroke at every level: Window 128,
Dialog 128, Flyout 32, Tooltip 16, **Card 8**, Control 2, Layer 1 (Verified, layering page). The
page is explicit that *"shadows work together with contour (outlines) to communicate depth"* and
that *"overusing shadows… can diminish their impact and create visual noise."* GMM's cards use a
1px border and no shadow, which is defensible; the Settings dialog's
`box-shadow: 0 1.5rem 3.75rem rgba(0,0,0,0.55)` is a 24px/60px shadow, far softer and larger than
a dialog at elevation 128 with a 1px stroke would be.

### 1.3 Spacing rhythm: a 4px base unit

Fluent 2's spacing ramp, with exact values (Verified, [Fluent 2 — Layout](https://fluent2.microsoft.design/layout)):

```
0, 2, 4, 6, 8, 10, 12, 16, 20, 24, 28, 32, 36, 40, 48, 52, 56
```

*"The base unit is four pixels."* The 2, 6 and 10 values exist only so Fluent icons land on that
grid. Gutters should be multiples of the base unit.

GMM's spacing is authored in `rem` against a 16px root: `0.5rem` (8px) ✓, `0.75rem` (12px) ✓,
`1.5rem` (24px) ✓, `1.25rem` (20px) ✓ — most values already land on the 4px grid. The outliers are
`0.45rem` (7.2px), `0.65rem` (10.4px), `0.6rem` (9.6px), `0.9rem` (14.4px), `0.35rem` (5.6px) and
`0.3rem` (4.8px). Those are the ones that make the rhythm feel soft rather than snapped.

`.card { padding: 1.25rem 1.5rem }` is 20px/24px, which is generous for a utility app — Notepad's
silhouette margin is 12 epx — but it is on the grid, so it is a judgement call, not a violation.

### 1.4 Type: the Windows 11 ramp, and how to get Segoe UI Variable without bundling it

Windows 11's UI face is **Segoe UI Variable**, *"a refreshed take on the classic Segoe"* that
*"uses variable font technology to dynamically provide great legibility at very small sizes, and
improved outlines at display sizes"* (Verified,
[Typography in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/typography)).
It has two axes: `wght` (100–700) and `opsz`, which is *"automatic and on by default… for optical
scaling from 8pt to 36pt."*

The sentence that matters for GMM is on that same page, and it is about HTML specifically:

> *"When using HTML, optical scaling is also automatic, but you will need to specify the Segoe UI
> Variable font in CSS."*

**So the correct way to reference it is by name in `font-family`, and nothing else.** The font ships
with Windows 11; naming it in CSS is all that is required, the optical axis then works on its own,
and no file is bundled or served. The current stack is (Verified, `src/App.css:2`):

```css
font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, system-ui, sans-serif;
```

On Windows that resolves to Segoe UI — the Windows 10 face — because Segoe UI Variable is never
named. Two macOS-only families lead the stack of a Windows-only application. Naming
`"Segoe UI Variable Text"` and `"Segoe UI Variable"` ahead of `"Segoe UI"` is the entire fix.

> **Do not bundle a Microsoft system font.** The licence forbids it for GMM's purposes; see
> [§3.6](#36-the-font-and-icon-licence-trap-the-one-real-licence-risk). An open-source,
> metrically-compatible alternative exists — Selawik, *"intended for apps on other platforms that
> don't want to bundle Segoe UI"* (Verified, same page) — but it is the wrong tool here. GMM runs
> only on Windows, where the real face is already installed; bundling a clone would make GMM look
> *less* like its host, not more.

The **Windows 11 type ramp**, verbatim (Verified, same page). *"All sizes are in effective pixels."*

| Style | Weight | Size / line height |
| --- | --- | --- |
| Caption | Small | 12 / 16 epx |
| **Body** | Text | **14 / 20 epx** |
| Body strong | Text semibold | 14 / 20 epx |
| Body large | Text | 18 / 24 epx |
| Body large strong | Text semibold | 18 / 24 epx |
| Subtitle | Display semibold | 20 / 28 epx |
| **Title** | Display semibold | **28 / 36 epx** |
| Title large | Display semibold | 40 / 52 epx |
| Display | Display semibold | 68 / 92 epx |

Three rules come with it (Verified, same page): minimum legible values are *"14px Semibold, 12px
Regular"*; casing is *"Sentence case… for all UI text, including titles"*; and *"Bold and Italic
styles are not part of the Windows type ramp. Use Semibold instead of Bold for emphasis."*

**GMM against the ramp** (Verified, `src/App.css`):

| Selector | GMM now | Nearest ramp step | Verdict |
| --- | --- | --- | --- |
| `.game-header h1` | 1.75rem = **28px**, `line-height: 1.2` → 33.6px | Title, 28/36 | size ✓, line height should be 36px |
| `.app__header h1` | 1.5rem = 24px | not in the ramp | → 20/28 (Subtitle) or 28/36 |
| `.onboarding__header h1` | 1.4rem = 22.4px | not in the ramp | → 20/28 |
| `.card h2`, `.attention-indicator__report h2` | 1.05rem ≈ 16.8px / 1rem = 16px | not in the ramp | → 16/22 or Subtitle 20/28 |
| `.muted`, `button`, `.game-status-strip` | 0.9rem / 0.85rem = 14.4 / 13.6px | Body, 14/20 | → exactly 14px |
| `.small`, `.attention-indicator__scope` | 0.75rem = **12px** | Caption, 12/16 | ✓ |
| `.conflict-pill`, `.update-pill` | 0.7rem = 11.2px | below the 12px minimum | **illegible by Microsoft's own rule** |
| `.variants` | 0.8rem = 12.8px | Caption 12 or Body 14 | pick one |

Nine distinct sizes, several of them fractional, two below the stated minimum. The ramp has nine
steps for the whole operating system.

### 1.5 Control metrics: the numbers Fluent actually ships

These come from Fluent UI React v9's own source, which is the reference implementation of Fluent 2
for the web, so the values are exact rather than eyeballed from a screenshot (Verified,
[`useButtonStyles.styles.ts`](https://github.com/microsoft/fluentui/blob/master/packages/react-components/react-button/library/src/components/Button/useButtonStyles.styles.ts)
and [`fonts.ts`](https://github.com/microsoft/fluentui/blob/master/packages/tokens/src/global/fonts.ts)):

| | Small | **Medium (default)** | Large |
| --- | --- | --- | --- |
| Vertical padding | 3px | **5px** | 8px |
| Horizontal padding | `spacingHorizontalS` = 8px | **`spacingHorizontalM` = 12px** | `spacingHorizontalL` = 16px |
| Font size / line height | 12 / 16 | **14 / 20** | 16 / 22 |
| Border | 1px | **1px** | 1px |
| **Computed height** | **24px** | **32px** | **40px** |
| `min-width` | 64px | **96px** | 96px |
| `border-radius` | 4px (`borderRadiusMedium`) | **4px** | 4px |
| Font weight | semibold (600) | **semibold** | semibold |

Icon-only buttons are square: `min-width` 24 / 32 / 40px (Verified, same file).

The resting appearance of a default Fluent button is **neutral, not accent**: `backgroundColor:
colorNeutralBackground1`, `color: colorNeutralForeground1`, `border: 1px solid colorNeutralStroke1`
(Verified, same file). The accent fill belongs to the `primary` appearance only.

**GMM's base `button` rule today** (Verified, `src/App.css`):

```css
button {
  background: #3b82f6;      /* saturated blue fill on EVERY button */
  color: white;
  border: none;             /* no 1px contour */
  padding: 0.5rem 0.9rem;   /* 8px / 14.4px */
  border-radius: 8px;       /* should be 4px */
  font-size: 0.9rem;        /* 14.4px, should be 14px */
}
```

Computed height is 8 + 8 + 20 = 36px, close to Fluent's 32px but not it; there is no border, the
radius is double, and — the loud one — **every button in the application is a filled accent
button**. "Close", "Browse", "Re-detect", "Rebuild junctions" and "Launch" are all the same
saturated blue. On Windows exactly one control per surface gets the accent.

### 1.6 Accent colour: sparingly, and you cannot read it from CSS

Microsoft's position is short and unambiguous (Verified,
[Color in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/color)):

> *"Accent color is used to emphasize important elements in the user interface and to indicate the
> state of an interactive object or control. Accent color values are generated automatically and
> optimized for contrast in both light and dark modes. **Accent colors are used sparingly** to
> highlight important elements and convey information about an interactive element's state."*

And: *"Color is personal. In Windows, users can choose an accent color and a light or dark theme…
You can choose how to incorporate the user's accent color and theme into your application."*

**Reading the user's accent colour from a WebView2 page is not currently reliable.** The CSS
system-colour keyword `AccentColor` is specified, and Firefox and Safari ship it, but Chromium
gated it behind a flag over fingerprinting and — Reported, from the Chromium tracker — enabled it
by default only for *installed web app* and initial-profile contexts, with the tracking issue
([40229450](https://issues.chromium.org/issues/40229450)) closed in April 2026. A Tauri WebView2
window is not an installed web app, so the keyword should be treated as unavailable without a
`@supports (color: AccentColor)` guard. I did not test this in WebView2; see
[§7](#7-what-we-could-not-establish).

The available route is the Rust side. `Windows.UI.ViewManagement.UISettings.GetColorValue(UIColorType)`
returns the system colour values and has shipped since Windows 10 1507 (Verified,
[API reference](https://learn.microsoft.com/en-us/uwp/api/windows.ui.viewmanagement.uisettings.getcolorvalue);
only `UIColorType.Complement` is unsupported and throws). Whether a Tauri desktop process can call
it cleanly, and how it would push changes to the page, is unverified.

**For a first slice this does not matter.** "Use one accent sparingly" is satisfied by picking a
single fixed blue for Launch and making every other button neutral. Following the *user's* accent
is a separate, later, and genuinely harder piece of work.

### 1.7 Window materials: what Mica actually requires

Mica is *"an opaque, dynamic material that incorporates theme and desktop wallpaper to paint the
background of long-lived windows such as apps and settings"* (Verified,
[Mica material](https://learn.microsoft.com/en-us/windows/apps/design/style/mica)). Mica Alt is a
stronger-tinted variant for apps with a tabbed title bar, and needs Windows App SDK 1.1+ on
Windows 11 22000+.

The three recommendations are short and they are the whole problem (Verified, same page):

> - **Do** set the background to *transparent* for all layers where you want to see Mica so the
>   Mica shows through.
> - **Don't** apply backdrop material more than once in an application.
> - **Don't** apply backdrop material to a UI element. The backdrop material will not appear on the
>   element itself. It will only appear if **all layers between the UI element and the window are
>   set to transparent**.

Mica also silently falls back to a solid colour (`SolidBackgroundFillColorBase`) when the user turns
off transparency, in Battery Saver, on low-end hardware, when the window deactivates, or on Windows
below build 22000 (Verified, same page). And Microsoft says *"Mica should be visible in the title
bar if you choose to apply the material"*, which means *"extending your app into the non-client area
and creating a transparent custom title bar"* — i.e. giving up the native title bar #326 just
earned.

**On the Tauri side**, from the config schema itself (Verified,
[`config.schema.json`](https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-schema-generator/schemas/config.schema.json)):

- `windowEffects` — *"Requires the window to be transparent."* Platform note: *"**Windows**: If
  using decorations or shadows, you may want to try this workaround
  [tao#72](https://github.com/tauri-apps/tao/issues/72#issuecomment-975607891)"*. *"**Linux**:
  Unsupported."*
- Windows effect values: `mica`, `micaDark`, `micaLight` — all *"**Windows 11 Only**"*; `tabbed`,
  `tabbedDark`, `tabbedLight` — Windows 11 only; `acrylic` — Windows 10 and 11, with *"bad
  performance when resizing or dragging"* on Windows 10 v1903+ and Windows 11 build 22000; `blur` —
  Windows 7/10/11 22H1, with the same resize/drag warning on 22621+.
- `WindowEffectsConfig.color` affects *"`Blur` and `Acrylic` only on Windows 10 v1903+. Doesn't have
  any effect on Windows 7 or Windows 11."* So Mica's tint is not configurable.
- `shadow` note: *"`false` has no effect on decorated window, shadow are always ON."*

GMM's `src-tauri/tauri.conf.json` today sets `width: 1100, height: 760, minWidth: 900,
minHeight: 600` and specifies neither `decorations` nor `transparent`, so it runs with the Tauri
defaults — `decorations: true`, `transparent: false` (Verified, both the repository file and the
schema's stated defaults). **Turning on Mica means turning on `transparent: true` on a decorated
window, which is the exact combination the schema flags as needing a workaround.**

The Rust-side alternative is the `window-vibrancy` crate: `apply_mica` (Windows 11),
`apply_acrylic` (Windows 10/11), `apply_blur` (Windows 7/10/11 22H1), with the same resize/drag
caveats and *no caveats listed for Mica* (Verified,
[README](https://github.com/tauri-apps/tauri-plugin-vibrancy)). Its own README still requires
`html, body { background: transparent }` and `"transparent": true`.

And then there is the webview. Reported, from Tauri issue
[#1564](https://github.com/tauri-apps/tauri/issues/1564) and a WPF-UI discussion: an opaque layer
anywhere in the stack hides the backdrop, and the webview's own background is a long-standing
source of exactly that. GMM's `:root` currently sets `background: #18181b` unconditionally — in both
colour schemes, deliberately, because the status bar's light-mode rule depends on the main screen
being dark under a light OS theme (Verified, the comment in `src/App.css` and the rule it guards,
held by `App.test.tsx`). That one declaration would have to go, and the status bar's documented
light-mode behaviour re-decided, before any Mica would be visible.

---

## 2. Option-by-option assessment, as #365 frames them

### Option A — layout and density pass only

**What changes.** `src/App.css`, and nothing else. Drop `.app`'s `max-width: 880px; margin: 0 auto`
and `.onboarding`'s `max-width: 760px`; left-align with a Windows-scale inline margin; retune the
`border-radius` values to 8/4/2; move every font size onto the Windows 11 ramp; name
`Segoe UI Variable` in `font-family`; make the base `button` neutral with a 1px contour and reserve
the accent fill for Launch; snap the off-grid spacing values to the 4px ramp. `.status-bar`'s
`padding-inline: calc(max(0px, (100% - 880px) / 2) + 1.5rem)` — which exists only to re-centre the
bar's content on the 880px column — becomes a plain inline padding matching the new page margin.

**What it buys.** The whole of #365's visible list. Item 1 (centred column) is the complaint itself.
Item 3 (web control vocabulary — pills, saturated fills, heading sizes, card padding) is entirely
CSS. Item 5 (the font stack) is one line. Item 2 (the dialog not reading as a dialog at the default
size) is partly addressed for free: the dialog reads as a dialog by contrast with the screen behind
it, and a screen that fills its window gives the dialog something to be inset *from*.

**What it costs.** One file. No markup, no component, no dependency, no new concept.

**What it risks.** Three things, all small and all nameable.

1. **The 880px assertion.** `src/App.test.tsx:583` asserts
   `expect(getComputedStyle(main).maxWidth).toBe("880px")`, with the comment *"`.app` is a centred
   880px column, so the bar cannot live inside it and still span the window"*. This is the one test
   that fails by construction. Its *purpose* — the bar is a sibling of `<main>`, not a child — is
   independently asserted two lines above (`main.nextElementSibling).toBe(bar)`), so the 880px line
   is restating the premise, not guarding behaviour. It should be replaced by the property it was
   standing in for, not deleted: the bar spans the window while its content lines up with the page
   margin.
2. **The clearance invariant.** `.app`'s `padding: 2rem 1.5rem 8rem` — the 8rem bottom is the fixed
   status bar's scroll clearance, and `App.test.tsx` holds `padding-bottom >= .status-bar`'s
   `max-height` (7rem). Any padding retune must keep that ordering. The test states it as an
   inequality, so it survives as long as the bottom padding stays at or above 7rem.
3. **Line length at a maximized window.** Removing the cap means a paragraph in a notice card can
   run the full width of a 2560px monitor, which breaks Microsoft's own 50–60-character rule. The
   fix is a measure on prose (`max-width: 60ch` on `p`, `.muted`, the notice bodies), not a measure
   on the page. This is the one place where "fill the window" needs a counterweight, and it is the
   correct one — Microsoft caps the paragraph, not the application.

**To reverse.** `git revert` on one file. Nothing outside `src/App.css` moves, so there is no
migration to unwind, no dependency to remove, and no API surface that other code came to depend on.
This is the cheapest option to undo of the four by a wide margin.

### Option B — adopt a Fluent component library

**What changes.** Every surface. `FluentProvider` at the root with a `webDarkTheme`/`webLightTheme`;
`button` → `Button`, `input` → `Input`/`Field`, `.card` → `Card`, `.game-tabs` → `TabList`/`Tab`,
`.mods` → `List` or a hand-rolled equivalent, the Settings navigation → `Nav` or `TabList`, and the
Settings dialog → `Dialog`/`DialogSurface`. `src/App.css` is largely replaced by Griffel
(`@griffel/react`, the CSS-in-JS engine Fluent compiles its styles through — Verified, imported by
`useButtonStyles.styles.ts`).

**What it buys.** A maintained control vocabulary with correct metrics by construction, keyboard and
screen-reader behaviour someone else maintains, and a token system (`tokens.borderRadiusMedium`,
`tokens.fontSizeBase300`) that makes "did we get the radius right" a non-question. That is real
value if GMM were going to grow a lot of surfaces.

**What it costs — and this is the part that decides it.**

1. **It does not fix the complaint.** Fluent components are controls. The centred column is
   `.app`'s `max-width`, which is GMM's layout, not Fluent's. #365 item 1 — *"the single most
   visible cause of the complaint"* — is untouched by B and still needs A's change.
2. **It does not fix the font either.** `@fluentui/react-theme`'s `fontFamilyBase` is
   `"'Segoe UI', 'Segoe UI Web (West European)', -apple-system, BlinkMacSystemFont, Roboto,
   'Helvetica Neue', sans-serif"` (Verified,
   [`fonts.ts`](https://github.com/microsoft/fluentui/blob/master/packages/tokens/src/global/fonts.ts)).
   **Segoe UI Variable does not appear.** Fluent UI React targets the web broadly, including
   Windows 10 and non-Windows browsers, so it reasonably ships the older face. Adopting B and then
   overriding `fontFamilyBase` to get item 5 is strictly more work than doing item 5 in A.
3. **It does not fix the material.** #365 item 4 is option C's territory, not B's.
4. **Install footprint: 32.3 MiB.** The root package's `unpackedSize` is 2,238,744 bytes, and it has
   **62 direct `@fluentui/*` dependencies**; summing the root plus all 62 at their current latest
   versions gives 33,860,007 bytes ≈ 32.3 MiB (Verified, computed from npm registry metadata on
   2026-10-14; largest single entries `@fluentui/react-table` 1.4 MiB, `@fluentui/react-menu` 1.4
   MiB, `@fluentui/react-tree` 1.2 MiB). That is install size, not shipped bundle size — the package
   declares `sideEffects: false` (Verified,
   [`package.json`](https://github.com/microsoft/fluentui/blob/master/packages/react-components/react-components/package.json)),
   so tree-shaking should cut it substantially. I could not measure the shipped delta without
   installing it; see [§7](#7-what-we-could-not-establish).
5. **The Settings dialog's three load-bearing properties have to be re-proved against someone
   else's component.** Covered in detail in [§4.3](#43-what-b-does-to-the-settings-dialogs-three-guarantees).
   The short version: Fluent's `Dialog` *can* reproduce all three, but only via two non-default
   props (`unmountOnClose={false}` and `inertTrapFocus`), its focus trap is Tabster rather than the
   dialog's own cycle, and Tabster's sentinel elements interact with GMM's own `tabStops()` helper.
6. **The three CSS-reading tests lose their subject.** `App.test.tsx`, `App.settings.test.tsx` and
   `AttentionView.test.tsx` each `readFileSync("src/App.css")` and assert against the authored text
   and the resulting cascade. Griffel generates hashed atomic classes at runtime; there is no
   authored stylesheet left to read.

**What it risks.** An ecosystem risk GMM does not currently carry. Fluent UI is actively maintained
— `microsoft/fluentui` was last pushed 2026-10-14, with release commits that same day, 20,320 stars
and 801 open issues (Verified, GitHub API, 2026-10-14) — so the risk is not abandonment. It is that
GMM's most carefully specified surface would depend on a third party's focus-management internals,
and those internals have open bugs: focus escaping a modal nested in a Menu
([#34103](https://github.com/microsoft/fluentui/issues/34103)), the trap failing with dialog actions
([#34101](https://github.com/microsoft/fluentui/issues/34101)), focus jumping to background elements
when the dialog has no focusable child ([#26829](https://github.com/microsoft/fluentui/issues/26829)),
and Tabster's root dummy inputs staying tabbable after close
([tabster#601](https://github.com/microsoft/tabster/issues/601)). Those are Reported — I read the
issue titles, not each thread to its end — but they are the right class of risk to name, because
#353 chose a hand-written focus cycle precisely so that *"one missing attribute on the window chrome
cannot let Tab walk into the screen behind."*

**To reverse.** Expensive and asymmetric. Every component rewritten once to adopt and once to back
out; `src/App.css` deleted and reconstructed; the three CSS-reading tests retired and then
reinstated; the dialog's containment re-proved twice. In practice B is a one-way door.

### Option C — native window materials only

**What changes.** `src-tauri/tauri.conf.json` gains `"transparent": true` and
`"windowEffects": { "effects": ["mica"] }`, or the Rust side calls `window_vibrancy::apply_mica`.
`:root`'s unconditional `background: #18181b` goes, `html, body` become transparent, and every
container that should show the material loses its background.

**What it buys.** The one thing A cannot give: #365 item 4. A window that genuinely participates in
the Windows 11 compositor, tints with the user's wallpaper, and goes neutral when the window
deactivates — the cue Microsoft says *"aid[s] productivity, by increasing clarity about which window
is in focus."*

**What it costs.** Small in lines, large in interactions:

- `transparent: true` on a window with native `decorations` is the combination Tauri's own schema
  flags with a workaround link (Verified, §1.7).
- Mica in the title bar requires *"extending your app into the non-client area and creating a
  transparent custom title bar"* (Verified) — i.e. undoing #326's real title bar. Without that, the
  material stops at the client area and the window reads as half-converted.
- `:root { background: #18181b }` must go, and the status bar's documented light-mode rule depends on
  it: the bar *"keeps the dark surface rather than borrowing `.settings-screen`'s light palette
  because `:root` sets `background: #18181b` in both schemes, so the main screen this bar borders is
  dark under a light OS theme too"* (Verified, the comment in `src/App.css`). `App.test.tsx` holds
  that the bar has *"a dark rule and a `prefers-color-scheme: light` rule"*, both with an explicit
  `background` and `color`. Removing the root background reopens the #326 dark-status-bar decision,
  which #365's constraints say stands.
- The scrim `rgba(9,9,11,0.62)` over a Mica base, and `.card`'s `#23232a` fill on top of it, would
  both need re-deriving. Microsoft's answer is `LayerFillColorDefaultBrush` — *"a low-opacity solid
  color"* for the content layer over Mica (Verified, Mica page) — so the cards become translucent
  neutrals rather than opaque greys, which is a visual decision, not a mechanical one.

**What it risks.** The honest risk is **doing all of that and seeing nothing.** Mica appears only
when every layer between it and the window is transparent (Verified, Microsoft's third
recommendation), it falls back to a flat colour under five common conditions including "the user
turned transparency off" and "battery saver" (Verified), and the WebView2 opaque-background problem
is a live Tauri issue (Reported, [#1564](https://github.com/tauri-apps/tauri/issues/1564)). None of
it is testable in jsdom, and #365's own acceptance criteria say *"whatever lands is verified on
Windows with screenshots, because no test here can establish it."* C is the option with the highest
ratio of unverifiable claims to lines changed.

And #365 already states the decisive objection: C *"decorates a layout that is still centred."* A
Mica backdrop behind an 880px centred column makes the empty 110px gutters *more* visible, not less,
because they would now be the only part of the window showing the wallpaper.

**To reverse.** Moderate. The config keys come back out cleanly, but the CSS background decisions
taken to let the material through — the root background, the card fills, the scrim, the status bar's
light-mode rule — are entangled with decisions #326 and #352 already recorded, and backing them out
means re-deciding them rather than reverting them.

### Option D — a combination, most plausibly A then C

**What changes.** A's CSS pass lands and ships. C is then evaluated against a window that already
fills itself and uses Windows metrics.

**What it buys.** Everything A buys, immediately, with C's question asked at the point where it can
be answered honestly. After A, "does this still not feel native?" is a real question with a small
answer space; before A, it is unanswerable because the centred column dominates every screenshot.

**What it costs.** A's cost now; C's cost later, if at all. The important property is that A's work
is not wasted if C is never done, and C's work is not *attempted* until there is something worth
decorating.

**What it risks.** Only the risk of two tickets instead of one. There is no technical coupling: A is
confined to `src/App.css`, C is confined to `src-tauri/tauri.conf.json` plus the background
declarations A would have already rationalised. If anything A makes C cheaper, because an A that
replaces scattered hard-coded hex fills with a small set of named surface colours leaves C with a
handful of values to make transparent instead of thirty.

**To reverse.** Each slice independently, in the reverse order it landed.

**D is the structurally correct answer, and A is its first slice.** #365 says *"the cheapest one may
be enough"*, and the evidence here is that it probably is: A covers four of the five numbered
observations in the issue, and the fifth (item 4, materials) is the only one that needs anything
else.

---

## 3. Candidate libraries assessed

All versions and dates from the npm registry and GitHub API on **2026-10-14**. Every licence is read
from the project's own `LICENSE` file in its own repository, not from a summary site.

### 3.1 The licence rule first

GMM is `GPL-3.0-or-later` (Verified, `src-tauri/Cargo.toml` workspace `license`, and ADR 0001). Two
permissive licences appear below, and both are explicitly GPLv3-compatible per the FSF's own licence
list (Verified, [gnu.org licence list](https://www.gnu.org/licenses/license-list.en.html)):

- **Expat / "MIT"**: *"This is a lax, permissive non-copyleft free software license, compatible with
  the GNU GPL."*
- **Apache License, Version 2.0**: *"This is a free software license, compatible with version 3 of
  the GNU GPL. Please note that this license is not compatible with GPL version 2."* GMM is GPLv3,
  so this is fine; a hypothetical future GPLv2 relicensing would not be.

So the code licence is not the constraint for any candidate below. **The asset licence is** — see
[§3.6](#36-the-font-and-icon-licence-trap-the-one-real-licence-risk).

### 3.2 `@fluentui/react-components` — Microsoft's Fluent 2 for React

| | |
| --- | --- |
| **Version** | **9.74.9**, published **2026-09-23**; the registry's `modified` timestamp for the package is 2026-10-14, so the line is being released against continuously |
| **Licence** | **MIT** — Verified twice. The repository root [`LICENSE`](https://github.com/microsoft/fluentui/blob/master/LICENSE) is verbatim "MIT License / Copyright (c) Microsoft Corporation", plus a trailing note (see §3.6). The package's own [`packages/react-components/react-components/LICENSE`](https://github.com/microsoft/fluentui/blob/master/packages/react-components/react-components/LICENSE) repeats MIT and the same note. Its `package.json` declares `"license": "MIT"`. **GitHub's API reports the repository licence as `NOASSERTION` / "Other"** — a detector artefact of the trailing asset note, not a different licence. |
| **Maintained** | **Yes, actively.** `microsoft/fluentui` last pushed 2026-10-14; the three most recent commits are dated 2026-10-14 and are component fixes (`react-charts`, `react-color-picker`, `web-components`). 20,320 stars, 801 open issues, not archived. (Verified, GitHub API) |
| **Install footprint** | Root 2,238,744 bytes + **62 direct `@fluentui/*` dependencies** = **33,860,007 bytes ≈ 32.3 MiB** unpacked (Verified, summed from the registry). `sideEffects: false` is declared, so the shipped bundle should be far smaller — unmeasured, see §7. |
| **Peer deps** | `react >=16.14.0 <20.0.0`; `@types/react` optional. GMM is on React 19.1 (Verified, `package.json`) — in range. |
| **Modal / dialog primitive** | **Yes.** `@fluentui/react-dialog`, with `Dialog`, `DialogSurface`, `DialogBody`, `DialogTrigger`. Assessed in detail in §4.3. |

### 3.3 `@fluentui/web-components` — Microsoft's Fluent 2 as custom elements

| | |
| --- | --- |
| **Version** | **3.1.3**, published **2026-08-25** |
| **Licence** | **MIT** — same `microsoft/fluentui` repository and root `LICENSE` as above; `package.json` declares MIT (Verified) |
| **Maintained** | **Yes** — same repository; the most recent commit I saw (2026-10-14) was in fact a `web-components` fix (Verified) |
| **Install footprint** | 5,249,121 bytes ≈ **5.0 MiB** unpacked, with only two dependencies (`@fluentui/tokens`, `tslib`) — dramatically leaner than the React line (Verified) |
| **Components** | 49 source directories including `dialog`, `dialog-body`, `drawer`, `button`, `text-input`, `field`, `tablist`, `tab`, `menu`, `tree`, `message-bar` (Verified, [`packages/web-components/src`](https://github.com/microsoft/fluentui/tree/master/packages/web-components/src)) |
| **Modal / dialog primitive** | **Yes — and it is built on exactly the mechanism #353 rejected.** |

**Why this one is disqualified, specifically.** `fluent-dialog`'s template wraps a **native
`<dialog>` element** inside a FAST custom element, and the component holds an `HTMLDialogElement`
reference (Verified,
[`dialog.template.ts`](https://github.com/microsoft/fluentui/blob/master/packages/web-components/src/dialog/dialog.template.ts)
and [`dialog.ts`](https://github.com/microsoft/fluentui/blob/master/packages/web-components/src/dialog/dialog.ts)).
#353 rejected a native `<dialog>` + `showModal()` for two stated reasons that both still hold: the
surface's `hidden` attribute is what retires it between visits and keeps the retained pages' drafts
alive (#309, #344), `showModal()` cannot be called on a hidden element, and **jsdom implements
neither**. On top of that, these are shadow-DOM custom elements, and Testing Library's
`getByRole`/`getByText` do not pierce shadow roots — which would invalidate the query style that
629 `getByRole` and 134 `getByText` call sites depend on. This is the wrong library for this
repository, for reasons the repository has already written down.

### 3.4 `@fluentui/react` v8 — the legacy line

| | |
| --- | --- |
| **Version** | **8.125.8**, published **2026-10-01** |
| **Licence** | MIT (Verified, same repository `LICENSE`; `package.json` declares MIT) |
| **Maintained** | Still receiving releases, but it is the previous generation; v9 (`react-components`) is the Fluent 2 implementation |
| **Install footprint** | **44,193,049 bytes ≈ 42 MiB** for the root package alone, before its 14 dependencies (Verified) |
| **Modal primitive** | Yes (`Modal`, `Dialog`) |

**Disqualified.** It implements Fluent 1, not Fluent 2, so it would not deliver Windows 11 metrics —
and it depends on `@fluentui/font-icons-mdl2`, which brings the asset-licence problem of §3.6
directly into GMM's dependency tree rather than leaving it optional.

### 3.5 Headless libraries, and whether a Fluent theme exists for them

The thought is reasonable: take a headless library for behaviour, style it to Windows 11 metrics
yourself. **No candidate ships a Fluent theme**, so this buys behaviour only — and GMM's one piece
of hard behaviour (the Settings dialog) is already written, mutation-proved, and specified in
`docs/design/main-screen.md`.

| Package | Version | Published | Licence (from its own `LICENSE`) | Unpacked | Dialog primitive? |
| --- | --- | --- | --- | --- | --- |
| `@radix-ui/react-dialog` | 1.3.1 | 2026-10-12 | **MIT** — ["MIT License / Copyright (c) 2022 WorkOS"](https://github.com/radix-ui/primitives/blob/main/LICENSE) | 107,250 B (**105 KiB** — by far the smallest) | Yes, with `aria-hidden` + `react-remove-scroll` + `react-focus-scope` |
| `react-aria-components` | 1.22.1 | 2026-10-09 | **Apache-2.0** — [full Apache 2.0 text](https://github.com/adobe/react-spectrum/blob/main/LICENSE) | 6,935,582 B (6.6 MiB) | Yes (`Modal`, `Dialog`) |
| `@ark-ui/react` | 5.39.3 | 2026-10-05 | **MIT** — ["MIT License / Copyright (c) 2024 Chakra Systems Inc."](https://github.com/chakra-ui/ark/blob/main/LICENSE) | 3,339,249 B (3.2 MiB), 69 `@zag-js/*` deps | Yes (`Dialog`) |
| `@base-ui-components/react` | **1.0.0-rc.0** | **2025-12-04** | **MIT** — ["MIT License / Copyright (c) 2019 Material-UI SAS"](https://github.com/mui/base-ui/blob/master/LICENSE) | 4,332,950 B (4.1 MiB) | Yes (`Dialog`) |
| `@fluentui/react-headless-components-preview` | 0.3.1 | 2026-09-24 | MIT (same `microsoft/fluentui` repo) | 3,423,739 B (3.3 MiB) | Unverified — a preview line at `0.x` |

Notes. `@base-ui-components/react` is still a release candidate and its latest publish is **ten
months old** (2025-12-04, with a package-level `modified` of 2026-07-15) — the slowest-moving
candidate here, and not something to put a load-bearing modal on. All four licences are
GPLv3-compatible per §3.1. Radix is the only one whose footprint is in the same order of magnitude
as the behaviour it supplies, and if GMM ever needed a replacement for its hand-written focus cycle
it would be the one to look at — but it supplies no Fluent styling, so it answers none of #365.

### 3.6 The font-and-icon licence trap — the one real licence risk

Both Fluent UI `LICENSE` files carry a trailing line after the MIT text (Verified, quoted exactly):

> *"NOTE: Usage of the fonts and icons referenced in Fluent UI React is subject to the terms listed
> at https://aka.ms/fluentui-assets-license"*

That link resolves to the **Microsoft Fabric Assets License Agreement** (November 2019). Its
clause 1 grants a *"non-exclusive, revocable, worldwide, non-assignable, non-transferable,
royalty-free license to use the Fabric Assets… exclusively for the following purposes"* (Verified,
text extracted from the PDF myself):

> *a. In connection with the use of a Microsoft API within the development of a software
> application, website, or product you create or a service you offer designed to provide access or
> interact with a Microsoft service or application ("Application")*
>
> *b. To illustrate that Application integrates with one or more Microsoft products and services.*

And clause 2 defines **"Fabric Assets"** as: *"a. Segoe font; b. Microsoft Office icons; c. Microsoft
Fabric icons."*

**What this means for GMM.** GMM is a mod manager for gacha games. It does not access a Microsoft
service and does not integrate with a Microsoft product, so neither permitted purpose applies. It is
also a revocable, non-transferable licence with a trademark-compliance and quality-review regime
attached (clauses 3–5), which does not sit alongside GPLv3 distribution. Three practical
conclusions:

1. **Never bundle Segoe UI or Segoe UI Variable.** This is already a constraint in #365; the Fabric
   Assets Agreement is the reason it is the right constraint. **Referencing the installed system
   font by name in `font-family` is not covered by this agreement at all** — nothing is downloaded,
   copied or redistributed; the user's own Windows installation renders the text. That is the route
   §1.4 recommends and it carries no asset-licence exposure.
2. **`@fluentui/react-components`'s own MIT grant is unaffected.** The note is scoped to *fonts and
   icons*; the component code is MIT and GPLv3-compatible. Using the components without Microsoft's
   icons is clean.
3. **`@fluentui/react-icons` is a separate, genuinely MIT package.** Version 2.0.344, published
   2026-10-09, `"license": "MIT"`, and its repository `microsoft/fluentui-system-icons` carries a
   plain [MIT `LICENSE`](https://github.com/microsoft/fluentui-system-icons/blob/main/LICENSE)
   ("Copyright (c) 2020 Microsoft Corporation") with **no asset note** (Verified). So Fluent *System*
   Icons are usable. Note its unpacked size: **174,128,591 bytes ≈ 166 MiB** — it is a per-icon
   module tree, so only what you import ships, but it is a large thing to put in `node_modules`.
   `@fluentui/font-icons-mdl2` (a v8 dependency) is the one to avoid, since MDL2 is a Microsoft icon
   *font*.

### 3.7 WinUI-styled CSS kits: there is no serious candidate

I searched npm for WinUI- and Fluent-styled CSS kits. Nothing usable exists for React (Verified,
npm registry search, 2026-10-14):

- **`unocss-preset-winui`** — version 0.0.7, last published **2024-05-12**, 44,538 B, MIT. A
  pre-1.0 UnoCSS preset untouched for roughly 29 months. Not a dependency to take.
- **`xaml-ui`** — 1.0.35, 2026-10-01. *"Angular version of Microsoft's WinUI component library."*
  Wrong framework.
- **`@progress/kendo-theme-fluent`** — a Fluent-styled theme, but for Kendo UI (Telerik), a
  commercial component suite. Wrong licensing shape for a GPLv3 project and wrong components.
- **Fluent-styled kits for other frameworks** exist (`fluent-svelte` and similar); all are the wrong
  framework.

**Why this is unsurprising.** WinUI 3 is a native XAML framework, not a web one, and Microsoft's own
web answer *is* Fluent UI. There is no third-party "WinUI CSS" worth GMM's dependency budget. The
practical consequence is good news for option A: **the Windows 11 metrics are public, exact, and
small enough to write by hand** — three radii, one 4px spacing ramp, nine type steps, three control
heights. That is roughly what §1 is.

---

## 4. Test impact

The frontend suite is **560 tests across 30 files, all passing** (Verified — I ran `pnpm test` in
this worktree on 2026-10-14: `Test Files 30 passed (30) / Tests 560 passed (560)`). The number comes
from the run, not from counting `it(` call sites; the static count is 341 because of `it.each`.

The suite queries almost entirely through the accessibility tree. Counted across all test files
(Verified, `grep`):

| Query | Call sites |
| --- | --- |
| `getByRole` | 629 |
| `findByRole` | 237 |
| `findByText` | 211 |
| `getByText` | 134 |
| `queryByRole` | 124 |
| `queryByText` | 97 |
| `getAllByRole` | 26 |
| `getByLabelText` | 17 |
| `findByDisplayValue` | 15 |
| `getByPlaceholderText` | 14 |
| everything else | ~13 |

Class-name coupling is small and deliberate: 8 `.dropzone`, 6 `.settings-screen`, 4 `.mods`, 4
`.app`, 2 `.status-bar`, 2 `.row`, 1 `.attention-indicator__scope`. Three files read the stylesheet
itself: `App.test.tsx`, `App.settings.test.tsx` and `AttentionView.test.tsx`.

### 4.1 What option A does to them

**Almost nothing, because A changes no markup.** Role and text queries are blind to CSS. The
specific exposures are:

| Location | Assertion | Under A |
| --- | --- | --- |
| `App.test.tsx:583` | `expect(getComputedStyle(main).maxWidth).toBe("880px")` | **Fails. The only test that does.** Replace it with the property it stood in for: the bar spans the window and its content aligns with the page margin. |
| `App.test.tsx` (same test) | `parseFloat(getComputedStyle(main).paddingBottom) >= parseFloat(pinned.maxHeight)` | Survives if `.app`'s bottom padding stays ≥ `.status-bar`'s 7rem `max-height`. It is an inequality, so retuning within that bound is free. |
| `App.test.tsx` (same test) | `[position, bottom, left, right] === ["fixed","0px","0px","0px"]`; `backgroundColor` matches `rgb(...)`; `borderTopWidth !== "0px"` | Survives. A must keep the bar opaque and bordered — which it should anyway. |
| `App.test.tsx:667` onwards | The bar needs *"a dark rule and a `prefers-color-scheme: light` rule"*, each with an explicit `background` and `color`; exactly 2 matching rules | Survives as long as A does not collapse the two rules. #365's constraints say the dark-status-bar decision stands, so A must not touch this. |
| `AttentionView.test.tsx:386` | loud and quiet attention indicators have identical `padding`, `fontSize`, `fontWeight`, `borderWidth`; loud `backgroundColor === "rgba(0, 0, 0, 0)"`; only `borderColor` differs | Survives if A keeps the indicator transparent and keeps both states the same size. A density retune must change both states together. |
| `App.settings.test.tsx:165–211` | `.settings-screen` is `hidden` when retired, `getComputedStyle(root).display === "none"`, **and** the authored text matches `/\[hidden\]\s*\{\s*display:\s*none\s*!important\s*;?\s*\}/` | Survives. A must not remove or weaken `[hidden] { display: none !important }` — the test explains why the regex exists: jsdom's own UA stylesheet would keep `display: none` even with the author rule deleted, so only reading the text catches it. |
| `App.settings.test.tsx:1224–1251` | scrim `[position, inset, display] === ["fixed","0px","grid"]`, `zIndex > 20`, `padding > 0`, `[overflow, overscrollBehavior] === ["auto","contain"]`; dialog `width === "min(1120px, 100%)"`, `maxHeight === "100%"`, `overflow === "auto"`, `resize === "none"` | Survives, provided A leaves the dialog's geometry alone. **A must not change the dialog's width, inset, or scroll behaviour** — those are #353's recorded decisions and the string comparison is literal. Changing the dialog's `border-radius` from 12px to 8px touches none of these. |
| `App.settings.test.tsx:1048–1083` | With the authored stylesheet loaded, every element in the inert regions computes `pointerEvents === "none"`, and `userEvent.click` rejects with `/pointer-events: none/` | Survives. A must not remove `[inert] { pointer-events: none }`. |

So: **one assertion to rewrite, and a short list of declarations A must not delete.** All of them are
documented in `src/App.css` with a comment saying why, and all of them are orthogonal to layout,
radius, type and colour.

### 4.2 What option B does to them

Materially worse, in four distinct ways.

**(a) The three CSS-reading tests lose their subject entirely.** `App.test.tsx`,
`App.settings.test.tsx` and `AttentionView.test.tsx` all do
`style.textContent = readFileSync("src/App.css", "utf8")` and then assert on both the authored text
and the resulting cascade. Fluent styles through Griffel (`@griffel/react` 1.7.8, MIT, published
2026-09-23 — Verified), which generates hashed atomic classes at runtime and injects them into
`document.head`. There is no authored stylesheet to read, and the computed values come from classes
whose names are build artefacts. The affected guards are not incidental — they include the
`[hidden] { display: none !important }` text assertion (whose comment says the rule's presence *"is
not observable from the rendered tree"*), the pointer half of inertness, the status bar's two
colour-scheme rules, the dialog's geometry, and the attention indicator's equal-size property.
Several of these were mutation-proved. Each would have to be re-expressed against Griffel or
dropped, and dropping them is losing coverage, not simplifying.

**(b) Accessible names and query types shift under the Fluent wrappers.** The 629 `getByRole` call
sites mostly survive, because Fluent renders real `<button>` and `<input>` elements and `TabList`
renders `role="tab"` (which `App.settings.test.tsx` already queries:
`findByRole("tab", { name: "Honkai: Star Rail" })`). Two groups do not survive cleanly:

- The **14 `getByPlaceholderText` and 3 `findByPlaceholderText`** call sites. Fluent's idiom is
  `Field` + `Label`, not a bare placeholder; `NetworkPanel` alone has three placeholder-only inputs
  (`"proxy URL"`, `"username (optional)"`, and a password field whose placeholder is *state* —
  `cfg.data?.passwordSet ? "(set — leave blank to keep)" : "password"` — and so is asserted on).
  Converting them to labels is the accessibility improvement, but it is a test rewrite either way.
- Anything that asserts on a control's exact `textContent`. `App.settings.test.tsx:1170` does
  `within(header).getAllByRole("button").map(b => b.textContent)` and expects exactly `["Close"]`.
  A Fluent `Button` with an icon slot would add text or an `aria-label` and change that list.

**(c) Tabster's sentinel elements collide with GMM's own `tabStops()`.** `SettingsDestination.tsx`
defines its own focusable-element query and filter:

```ts
const FOCUSABLE = "a[href], button, input, select, textarea, summary, [tabindex]";
// …filtered by: not disabled, tabindex >= 0, type !== "hidden",
//               aria-hidden !== "true", closest("[hidden], [inert]") === null
```

Fluent's Dialog *"will use **Tabster** to handle the keyboard navigation and ensure focus trapping"*
(Verified, [Dialog Spec.md](https://github.com/microsoft/fluentui/blob/master/packages/react-components/react-dialog/library/docs/Spec.md)).
Tabster works by inserting sentinel "dummy input" elements with `tabindex` into the DOM — and
Tabster's own issue [#601](https://github.com/microsoft/tabster/issues/601) reports that *"root
dummy inputs stay tabbable (`tabindex="0"`) after a modal Dialog closes"*, triggering an axe
`aria-hidden-focus` failure (Reported). GMM's `[tabindex]` selector matches those sentinels. The
30-press Tab loop in `App.settings.test.tsx:1086–1128` asserts, on every press, that focus is inside
the dialog, not in `main`, not in the status bar, and not in the retained `hidden` page, and that
`visited.size > 1`. With Tabster in play that loop is asserting over two focus managers at once.

**(d) The dialog's three guarantees need reproducing with two non-default props.** Next section.

### 4.3 What B does to the Settings dialog's three guarantees

#365 states the constraint: *"The Settings dialog's inertness, focus containment and `hidden`
retention (#353, #309, #344) are load-bearing. No component library's own modal may quietly replace
them without reproducing all three."* Here is exactly where Fluent's `Dialog` stands on each, read
from its source.

| Guarantee | Fluent's default | Can Fluent do it? |
| --- | --- | --- |
| **`hidden`-based retention** — the Settings scrim stays mounted and `hidden`, so visited pages keep their drafts, pending operations and feedback (#309, #344) | **No.** `unmountOnClose` defaults to **`true`** (Verified, [`useDialog.ts`](https://github.com/microsoft/fluentui/blob/master/packages/react-components/react-dialog/library/src/components/Dialog/useDialog.ts): `unmountOnClose = true`), wired to `unmountOnExit` on the surface motion slot. Closing the dialog destroys every retained page's state. | **Yes, with `unmountOnClose={false}`.** The prop's own doc comment is almost a description of GMM's requirement: *"Decides whether the dialog should be removed from the DOM tree when it is closed. This can be useful when dealing with components that may contain state that should not be reset when the dialog is closed."* (Verified, `Dialog.types.ts`) |
| **Inertness of the window behind** — `inert` + `aria-hidden` on `main` and the status bar | **No.** `inertTrapFocus` defaults to **`false`** (Verified, `useDialog.ts`), so the default trap is Tabster-based, not `inert`-based. | **Partly.** `inertTrapFocus` *"Enables standard behavior according to the HTML dialog spec… where the focus trap involves setting outside elements inert"* (Verified, `Dialog.types.ts`). But GMM's `inert`/`aria-hidden` are applied by **GMM's own code** (`App.tsx` passes `inert={behind} aria-hidden={behind \|\| undefined}` to `<main>` and `<footer>`), and the tests assert those attributes on those two specific elements. Fluent would be managing a parallel mechanism over the same DOM. |
| **Focus containment** — the dialog's own Tab/Shift+Tab cycle, which wraps, skips `hidden` retained pages, and pulls focus back in from outside | Tabster. *"The tab sequence is kept within the dialog"* for modal dialogs; *"Windows under a modal dialog are inert"* (Verified, Spec.md). | **Probably, with open questions.** Tabster has reported failures in nested-Menu dialogs (#34103), dialogs with actions (#34101), and dialogs with no focusable child (#26829) — Reported. More to the point, #353 wrote GMM's cycle deliberately: *"a single missing attribute on the window chrome would let Tab walk out of the dialog and into the screen behind it. The cycle below is the dialog's own guarantee, independent of what the surrounding window did."* Handing that to Tabster gives up exactly the independence #353 was buying. |

**The honest summary.** Fluent's Dialog is not a blocker — it has a prop for each requirement, and
one of them was clearly added for this situation. But both props are non-default, the combination
(`unmountOnClose={false}` **and** `inertTrapFocus`) appears in no documentation I found, **jsdom
implements neither `inert` nor `showModal()`** so the composition could not be proved in this
repository's test environment, and GMM would be trading a 40-line focus cycle it owns and has
mutation-proved for a third party's focus manager with open trap bugs. That is a bad trade for the
single most carefully specified surface in the application.

### 4.4 What option C does to them

Less than B, more than zero. Three places:

- `:root { background: #18181b }` must go for Mica to show. `App.test.tsx`'s status-bar test asserts
  the bar's `backgroundColor` matches `rgb(...)` — i.e. is opaque — and that the authored stylesheet
  has exactly two `.status-bar` rules, one dark and one under `prefers-color-scheme: light`, each
  with a `background` and a `color`. Both survive a transparent root, but the *reason* the
  light-mode rule exists ("the main screen this bar borders is dark under a light OS theme too")
  stops being true, so the rule becomes unexplained rather than wrong.
- `.settings-screen`'s scrim `rgba(9,9,11,0.62)` and `.card`'s `#23232a` fill would be re-derived
  against Microsoft's `LayerFillColorDefaultBrush` guidance. No test asserts those colours, so this
  is free test-wise and expensive design-wise.
- **Nothing about C is testable here at all.** jsdom has no compositor, no DWM, and no WebView2. The
  entire option is a Windows screenshot check, which is what #365's acceptance criteria already say.

---

## 5. The centred-column question, answered

**Replace `max-width: 880px; margin: 0 auto` with a full-width, left-aligned layout and a fixed
inline margin. Put the measure on the paragraphs, not on the page. Do not introduce a second pane.**

### 5.1 Why the current rule is the complaint

`.app { max-width: 880px; margin: 0 auto; padding: 2rem 1.5rem 8rem }` (Verified, `src/App.css`).
The content box is therefore 880 − 48 = 832px however wide the window is. At GMM's window sizes
(Verified, `src-tauri/tauri.conf.json`):

| Window width | Empty ground per side | How it reads |
| --- | --- | --- |
| 900 (the minimum) | (900 − 880) / 2 = **10px** | Fine — the cap is invisible |
| **1100 (the default)** | (1100 − 880) / 2 = **110px** | **The complaint.** 20% of the window is empty on purpose |
| 1920 (maximized, 1080p) | (1920 − 880) / 2 = **520px** | 54% of the window is empty; the application looks like a web page in a browser |
| 2560 (maximized, 1440p) | (2560 − 880) / 2 = **840px** | 66% empty |

The first-run route is narrower still: `.onboarding { max-width: 760px; margin: 0 auto }`. So the
very first thing a user sees is the most centred thing in the application.

And the cap has already cost the codebase something. `.status-bar` carries
`padding-inline: calc(max(0px, (100% - 880px) / 2) + 1.5rem)` purely to re-centre its content on a
column it is not inside — a 40-line comment in `src/App.css` explains the arrangement. Removing the
cap deletes that calculation rather than complicating it.

### 5.2 Why a *wider* measure is the wrong answer

Raising 880px to 1200px or 1400px moves the breakpoint at which the window looks wrong; it does not
change the kind of thing the layout is. A maximized window on any modern monitor still shows empty
gutters, and the user still sees a document rather than an application. More importantly, **no
Windows 11 silhouette centres a column at any width** (§1.1), and Fluent 2's layout page declines to
publish a maximum content width at all (Verified). A wider cap is the same mistake with a larger
number.

### 5.3 Why a two-pane layout is the wrong answer *now*

It is the most tempting answer — a left nav pane is the most recognisably Windows 11 shape there is,
and Microsoft's own example of the left-navigation silhouette is Settings. But it is foreclosed in
this repository, twice over:

- `docs/design/main-screen.md` records that the main screen has **no sidebar and no dock** (#282,
  #283), and that *"its sidebar prohibition applies only to the main screen"* — i.e. the
  prohibition is deliberate and scoped, not an oversight.
- [#327](https://github.com/Derek-X-Wang/gmm/issues/327) already owns the docked panel: *"The docked
  panel was a candidate for Settings in #326 and lost to the modal dialog. It is not discarded. It
  is reassigned to the selected Mod's detail in #327."* The same document lists *"Mod inspector and
  the main-screen no-sidebar rule"* as an **open item needing a human decision**, with the note that
  *"#327 must reconcile the two when it is specified."*

So introducing a second pane here would pre-empt an open decision and contradict a recorded one.
`docs/design/main-screen.md` also says, plainly: *"Do not reopen the Settings presentation to reclaim
it."* A visual pass must not quietly become an information-architecture change.

**A does not block a two-pane layout later.** Left-aligning and filling the window is the
prerequisite for a nav pane, not an alternative to it: a pane can only be docked against a layout
that reaches the window edge. If #327 lands a panel, it docks into an A-shaped layout with no
further work.

### 5.4 The recommended replacement, concretely

```css
.app {
  /* was: max-width: 880px; margin: 0 auto; */
  max-width: none;
  margin: 0;
  padding: 1.5rem 1.5rem 8rem;   /* 24px inline; Notepad's silhouette uses 12epx,
                                    GMM's card content justifies the wider end */
}
```

and the same treatment for `.onboarding`, and

```css
.status-bar {
  /* was: padding-inline: calc(max(0px, (100% - 880px) / 2) + 1.5rem); */
  padding-inline: 1.5rem;        /* matches .app, so the two still line up */
}
```

with a measure moved onto prose rather than the page:

```css
.card p, .muted, .av-guidance p, .library-audit-warning p, .onboarding__welcome p {
  max-width: 68ch;               /* Microsoft: 50–60 letters per line, never over 60 */
}
```

`68ch` is approximately 60 letters at 14px — `ch` is the width of `0`, which runs a little narrower
than average lowercase, so `60ch` would land under Microsoft's floor. The exact number is a
judgement to make on a Windows screenshot, which is where #365 already puts it.

### 5.5 What this does to each existing surface

**At 900×600 (the minimum).** Almost nothing changes, because the cap was nearly inactive: content
gains 10px per side. Every surface already reflows here — `.mod-import-routes` uses
`repeat(auto-fit, minmax(min(100%, 16rem), 1fr))`, `.settings-layout` reflows at 600px,
`.game-tabs` wraps, `.game-status-strip` wraps with `flex: 1 1 260px` on its summary (Verified,
`src/App.css`). The Settings dialog is unaffected: it is sized against the viewport
(`width: min(1120px, 100%)` inside a 2rem scrim), not against `.app`, so it stays the documented
836×536 panel with 32px of main screen visible on every side.

**At 1100×760 (the default).** The 110px gutters disappear and every surface gets ~220px more width.
Concretely:

- `.mods__row` is `display: flex; justify-content: space-between` — the Mod name goes left, the
  controls go right, and the gap between them widens. This is the surface that gains most, because a
  mod list is exactly what extra width is for.
- `.mod-import-routes` is an `auto-fit` grid with a 16rem (256px) minimum. At 832px of content it
  fits 3 columns; at ~1052px it fits 4. Fine either way, and better.
- `.game-tabs` currently wraps at six games; with ~220px more it is likelier to sit on one row.
- `.duplicate-mod-details` uses `grid-template-columns: minmax(8rem, max-content) minmax(0, 1fr)`,
  so the value column absorbs the extra width — good, since those values are filesystem paths with
  `overflow-wrap: anywhere`.
- **The thing to watch:** `.card` is a single-column grid, so a card with one short paragraph becomes
  a very wide, very short box. This is where §5.4's prose measure earns its place, and where a
  Windows screenshot is needed to judge whether some cards also want a `max-width`.

**At 1920 and above (maximized).** The gain is the whole point — 520px per side of returned space —
and the risk is the same wide-and-short card shape, more acutely. Two defences, both already
natural: the prose measure above, and the fact that GMM's widest surfaces (the Mod list, the
duplicate report, the path fields) are tables and rows that genuinely use horizontal space. If a
maximized window still looks sparse after A, *that* is the moment to reconsider a two-pane layout —
with #327's decision made, not around it.

**The Settings dialog is untouched by all of this**, which is worth stating plainly because #365
raises it as item 2. Its sizing is a recorded #353 decision, asserted literally in
`App.settings.test.tsx` (`width === "min(1120px, 100%)"`), and it is measured against the viewport
rather than the page. What changes is the dialog's *context*: at 1100px wide today the dialog is a
1102px-ish panel over a screen whose content occupies only the middle 880px, so the "32px frame" is
a frame around emptiness. With the main screen filling its window, the same 32px inset frames actual
content, and the dialog reads as a dialog over a screen for the first time — at the default size,
without changing a single declaration on `.settings-dialog`.

---

## 6. Recommendation and a first slice

### 6.1 Recommendation

**Take option D with A as the first slice, and treat C as a separate, later, screenshot-gated
question. Do not take B.**

The reasoning in one line each:

- **A is the only option that addresses the complaint.** #365's item 1 is `.app`'s `max-width`, which
  only A touches, and the issue itself calls it *"the single most visible cause."*
- **A also covers items 3 and 5**, the control vocabulary and the font face, because both are CSS.
- **B covers item 3 only, at the cost of every surface** — and, decisively, Fluent's own
  `fontFamilyBase` is `'Segoe UI'`, not Segoe UI Variable (Verified), so B does not even deliver
  item 5. It is more work for a strict subset of A's result, plus a 32 MiB install, plus handing the
  Settings dialog's three mutation-proved guarantees to a third party's focus manager.
- **C covers item 4 only, and #365's own framing is right:** it decorates a layout that is still
  centred. It also requires `transparent: true` on a decorated window (which Tauri's schema flags),
  giving up the native title bar for Mica in the title bar (which Microsoft requires), and removing
  `:root`'s background (which reopens #326's status-bar decision). Each is survivable; all three at
  once, for an effect that falls back to a flat colour under five common conditions, is not a first
  slice.
- **Item 2 improves for free under A**, as §5.5 explains.

### 6.2 The first slice

**One file: `src/App.css`. One test assertion rewritten. No new dependency.**

| # | Change | Why, with its source |
| --- | --- | --- |
| 1 | `.app`: drop `max-width: 880px` and `margin: 0 auto`; inline padding 1.5rem. Same for `.onboarding` (drop `760px`). `.status-bar`: `padding-inline: 1.5rem`, dropping the `(100% - 880px) / 2` re-centring. | The complaint. No Windows 11 silhouette centres a column (§1.1). |
| 2 | Put a measure on prose: `max-width: ~68ch` on paragraph-level text. | Microsoft caps the line, not the page: *"50–60 letters per line"* (§1.1). |
| 3 | `font-family`: `"Segoe UI Variable Text", "Segoe UI Variable", "Segoe UI", system-ui, sans-serif`. Drop the macOS-first families from a Windows-only application. | *"When using HTML, optical scaling is also automatic, but you will need to specify the Segoe UI Variable font in CSS"* (§1.4). Nothing is bundled. |
| 4 | Radii → three values. `.card` 12 → **8**; `.settings-dialog` 12 → **8**; `button`, `.mods__row`, `.onboarding__*-row`, `input`, `.dropzone`, `.game-tabs__tab` → **4**; `.conflict-pill`, `.update-pill` → **2**. | `OverlayCornerRadius` 8px, `ControlCornerRadius` 4px, 2px under 32px (§1.2). Seven radii become three. |
| 5 | Type onto the ramp. `.muted` / `button` / `.game-status-strip` → **14px / 20px**; `.card h2` → **16/22** or **20/28**; `.app__header h1` → **20/28**; `.game-header h1` keep **28px** but line-height **36px**; pills **12px**, not 11.2px. | The Windows 11 type ramp, and its own minimum of *"12px Regular"* (§1.4). |
| 6 | `button` becomes neutral: surface fill, `1px solid` contour, `border-radius: 4px`, `padding: 5px 12px`, `font-size: 14px`, `line-height: 20px`, `font-weight: 600` → 32px tall. Accent fill stays on **`.game-header__launch` only**. | Fluent's own default button is neutral with a 1px stroke; accent is the `primary` appearance (§1.5), and *"accent colors are used sparingly"* (§1.6). |
| 7 | Snap the off-grid spacing (`0.3`, `0.35`, `0.45`, `0.6`, `0.65`, `0.9` rem) to the 4px ramp. | *"The base unit is four pixels"* (§1.3). |
| 8 | Rewrite `App.test.tsx:583`. `expect(getComputedStyle(main).maxWidth).toBe("880px")` asserts the premise, not the behaviour; the behaviour — the bar is a sibling of `<main>` and spans the window — is already asserted on the lines above and below it. | §4.1. |

**Declarations the slice must not touch** (each has a comment in `src/App.css` saying why, and a test
holding it): `[hidden] { display: none !important }`; `[inert] { pointer-events: none }`;
`.settings-screen`'s `position/inset/z-index/display/overflow/overscroll-behavior` and its 2rem
padding; `.settings-dialog`'s `width: min(1120px, 100%)`, `max-height: 100%`, `overflow: auto`;
`.status-bar`'s `position: fixed`, opaque background, `border-top`, `max-height: 7rem`, and its two
colour-scheme rules; `.app`'s bottom padding staying ≥ 7rem; `.attention-indicator__toggle`'s
transparent background and equal metrics in both states.

**Verification.** `pnpm test` must be 560 green with exactly one assertion rewritten. Then the part
no test can establish, and the part #365 already asks for: Windows screenshots at **900×600**,
**1100×760** and **maximized**, each with the main screen and with the Settings dialog open, light
theme and dark.

### 6.3 What the first slice deliberately does not do

- **No component library.** Not `@fluentui/react-components`, not a headless library. The Windows 11
  metrics are public and small; §1 is most of the specification.
- **No window material.** No `transparent: true`, no Mica, no Acrylic, no `window-vibrancy`. `:root`'s
  background stays, so #326's dark status bar is untouched.
- **No bundled font.** Segoe UI Variable is referenced by name and nothing else. No Selawik.
- **No two-pane layout and no nav rail.** #282/#283's no-sidebar rule stands and #327 owns the panel.
- **No change to the Settings dialog's geometry, sizing, inertness, focus cycle or retention.** #353's
  decisions stand; only the dialog's `border-radius` moves, 12px → 8px.
- **No accent-colour following.** One fixed accent on Launch. Reading the user's Windows accent is a
  separate piece of work with an unverified route (§1.6, §7).
- **No remembered window size or position.** Still an open decision in
  `docs/design/main-screen.md`.
- **No change to any string, any role, any label, or any behaviour.** If the slice changes what the
  application *says* or *does*, it has gone out of scope.

---

## 7. What we could not establish

Honest gaps. Nothing below is guessed at elsewhere in this document.

| # | Gap | Why not, and what would close it |
| --- | --- | --- |
| 1 | **Fluent UI's actual shipped bundle cost for GMM.** I have the install footprint — 32.3 MiB unpacked across the root and 62 direct dependencies (Verified) — and `sideEffects: false` (Verified), but not a measured production bundle delta. | The dispatch forbids installing it, correctly. Closing this needs a throwaway branch: `vite build` with and without a `FluentProvider` + the ~8 components GMM would actually use. |
| 2 | **Whether `unmountOnClose={false}` and `inertTrapFocus` compose correctly.** Both props exist and each addresses one of GMM's requirements (Verified, `Dialog.types.ts`). The combination appears in no documentation I found, and jsdom implements neither `inert` nor `showModal()`, so it cannot be proved in this repository's test environment. | A Windows/WebView2 manual check, or a Playwright test GMM does not currently have. |
| 3 | **Whether Mica is visible at all in a Tauri + WebView2 window with native decorations.** The Tauri schema flags `transparent` + decorations (Verified), and the webview's opaque background is a live issue (Reported, [#1564](https://github.com/tauri-apps/tauri/issues/1564)). I found no first-hand confirmation either way. | A Windows spike: `transparent: true` + `effects: ["mica"]` + a transparent `html, body`, screenshotted. One afternoon, and it is the only way to know. |
| 4 | **Whether the CSS `AccentColor` keyword resolves in WebView2.** Chromium gated it for fingerprinting and — Reported — enabled it only for installed-web-app contexts, with the tracking issue closed April 2026. A Tauri window is not an installed web app. I did not test it. | `@supports (color: AccentColor)` plus a swatch, in a GMM dev build on Windows. |
| 5 | **Whether a Tauri Rust process can read the accent colour via `UISettings.GetColorValue`.** The API exists and has shipped since Windows 10 1507 (Verified), but whether it is callable from GMM's process without a WinRT activation problem, and how changes would be pushed to the page, is unverified. | A small Rust spike against the `windows` crate. |
| 6 | **The exact `ch` value for Microsoft's 50–60-letter rule.** `68ch` is my arithmetic, not a measurement; `ch` is the advance of `0`, which differs from average lowercase, and it differs again between Segoe UI and Segoe UI Variable. | Measure it on a Windows screenshot. This is a judgement, not a fact, and belongs where #365 puts it. |
| 7 | **Whether the Fluent Dialog focus-trap issues (#34103, #34101, #26829, tabster#601) are open, fixed, or irrelevant to GMM's shape.** I read titles and search excerpts, not threads. | Read the four threads. Only matters if B is reconsidered. |
| 8 | **Whether a WinUI-styled CSS kit exists outside npm.** I searched the npm registry and the web; I did not search GitHub exhaustively for an unpublished kit. | Given that Microsoft's own web answer is Fluent UI, I judged further searching low-value. Stated so the gap is visible rather than implied. |
| 9 | **Fluent 2's published control *heights*.** The `fluent2.microsoft.design` button usage page 404'd when I fetched it. The 24 / 32 / 40px figures in §1.5 are my arithmetic from Fluent UI React's own source constants (`buttonSpacingSmall` 3px, `Medium` 5px, `Large` 8px; `fontSizeBase200/300/400` 12/14/16px; `lineHeightBase200/300/400` 16/20/22px; 1px border) — primary source, derived number. | Fetch the Fluent 2 button page when it is reachable, and compare. |
| 10 | **Nothing was verified on Windows.** Every rendered claim in this document is reasoning from Microsoft's published metrics and this repository's CSS. The 900×600, 1100×760 and maximized appearances are *predicted*, not seen. | #365's acceptance criteria already require this: *"whatever lands is verified on Windows with screenshots, because no test here can establish it."* |

**A procedural note, not a gap.** The dispatch asked me to run `pnpm lint` if the repository has a
docs lint. **It does not.** `package.json` declares no `lint` script (Verified), and CI's `script-lint`
job lints CI's own PowerShell and shell scripts via `.github/scripts/lint-ci-scripts.ps1` and
`actionlint`, not Markdown (Verified, `.github/workflows/ci.yml`). So: **NOT-RUN — no docs lint
exists in this repository.** `pnpm test` was run instead and is 560 green.

---

## Sources

Microsoft design guidance (all Verified by reading the page):

- [Geometry in Windows 11](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/geometry) — the 8 / 4 / 0 radii, `ControlCornerRadius`, `OverlayCornerRadius`
- [Typography in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/typography) — Segoe UI Variable, the optical axis, the type ramp, left alignment, 50–60 letters per line, Selawik
- [Layering and elevation in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/layering) — the two-layer system, elevation values
- [Windows app silhouettes](https://learn.microsoft.com/en-us/windows/apps/design/basics/app-silhouette) — the four silhouettes and their content margins
- [Color in Windows](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/color) — accent colour used sparingly
- [Mica material](https://learn.microsoft.com/en-us/windows/apps/design/style/mica) — Mica, Mica Alt, the three recommendations, the five fallback conditions
- [Fluent 2 — Layout](https://fluent2.microsoft.design/layout) — the spacing ramp, the 4px base unit
- [Fluent 2 — Shapes](https://fluent2.microsoft.design/shapes) — the radius tokens
- [`UISettings.GetColorValue`](https://learn.microsoft.com/en-us/uwp/api/windows.ui.viewmanagement.uisettings.getcolorvalue) — reading system colours

Library primary sources (Verified by reading the file):

- [microsoft/fluentui `LICENSE`](https://github.com/microsoft/fluentui/blob/master/LICENSE) and [`packages/react-components/react-components/LICENSE`](https://github.com/microsoft/fluentui/blob/master/packages/react-components/react-components/LICENSE)
- [Microsoft Fabric Assets License Agreement (Nov 2019)](https://aka.ms/fluentui-assets-license) — the Segoe-font and icon terms
- [microsoft/fluentui-system-icons `LICENSE`](https://github.com/microsoft/fluentui-system-icons/blob/main/LICENSE) — plain MIT, no asset note
- Fluent token and component source: [`borderRadius.ts`](https://github.com/microsoft/fluentui/blob/master/packages/tokens/src/global/borderRadius.ts), [`fonts.ts`](https://github.com/microsoft/fluentui/blob/master/packages/tokens/src/global/fonts.ts), [`useButtonStyles.styles.ts`](https://github.com/microsoft/fluentui/blob/master/packages/react-components/react-button/library/src/components/Button/useButtonStyles.styles.ts)
- Fluent Dialog: [`Dialog.types.ts`](https://github.com/microsoft/fluentui/blob/master/packages/react-components/react-dialog/library/src/components/Dialog/Dialog.types.ts), [`useDialog.ts`](https://github.com/microsoft/fluentui/blob/master/packages/react-components/react-dialog/library/src/components/Dialog/useDialog.ts), [Spec.md](https://github.com/microsoft/fluentui/blob/master/packages/react-components/react-dialog/library/docs/Spec.md)
- Fluent web-components Dialog: [`dialog.template.ts`](https://github.com/microsoft/fluentui/blob/master/packages/web-components/src/dialog/dialog.template.ts)
- [radix-ui/primitives `LICENSE`](https://github.com/radix-ui/primitives/blob/main/LICENSE), [adobe/react-spectrum `LICENSE`](https://github.com/adobe/react-spectrum/blob/main/LICENSE), [mui/base-ui `LICENSE`](https://github.com/mui/base-ui/blob/master/LICENSE), [chakra-ui/ark `LICENSE`](https://github.com/chakra-ui/ark/blob/main/LICENSE)
- [FSF licence list](https://www.gnu.org/licenses/license-list.en.html) — Expat and Apache-2.0 GPL compatibility
- Versions, dates and sizes: the npm registry and the GitHub API, queried 2026-10-14

Tauri (Verified):

- [Tauri config JSON schema](https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-schema-generator/schemas/config.schema.json) — `windowEffects`, `transparent`, `decorations`, `shadow`, the `WindowEffect` enum
- [tauri-apps/tauri-plugin-vibrancy README](https://github.com/tauri-apps/tauri-plugin-vibrancy) — `apply_mica` / `apply_acrylic` / `apply_blur` and their platform notes; crate `window-vibrancy` 0.8.1, 2026-09-22, `Apache-2.0 OR MIT` (crates.io API)

Reported (read as titles, excerpts or third-party summaries, not verified end to end):

- [tauri#1564](https://github.com/tauri-apps/tauri/issues/1564) — webview background vs. window material
- [tao#72 (comment)](https://github.com/tauri-apps/tao/issues/72#issuecomment-975607891) — the decorations + transparency workaround Tauri's schema links
- [fluentui#34103](https://github.com/microsoft/fluentui/issues/34103), [#34101](https://github.com/microsoft/fluentui/issues/34101), [#26829](https://github.com/microsoft/fluentui/issues/26829), [tabster#601](https://github.com/microsoft/tabster/issues/601) — Fluent Dialog / Tabster focus-trap reports
- [Chromium 40229450](https://issues.chromium.org/issues/40229450) — the `AccentColor` system-colour keyword's gating

This repository (Verified by reading the file or running the command):

- `src/App.css`, `src/App.tsx`, `src/SettingsDestination.tsx`, `src/App.settings.test.tsx`,
  `src/App.test.tsx`, `src/AttentionView.test.tsx`, `src-tauri/tauri.conf.json`,
  `src-tauri/Cargo.toml`, `package.json`, `.github/workflows/ci.yml`,
  `docs/design/main-screen.md`, `CONTEXT.md`, `docs/adr/`
- `pnpm test` — 30 files, 560 tests, all passing, 2026-10-14
