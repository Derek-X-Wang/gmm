# GMM icon candidates — issue #273

Original icon concepts generated with the built-in image generation tool for [issue #273](https://github.com/Derek-X-Wang/gmm/issues/273). The maintainer selected **A — Modular G**.

| Candidate | File | Idea |
| --- | --- | --- |
| A — Modular G | `a-modular-g.png` | A bold G assembled from blue modules. Recommended for a distinct app identity. |
| B — Library layers | `b-library-layers.png` | Three stacked cards represent the central mod Library. |
| C — Connected modules | `c-connected-modules.png` | Two blue pieces form an M for mods and their connections. |

The generated originals are 1254×1254 PNG files with an alpha channel. The prompts requested 1024×1024; the tool returned 1254×1254. The outside padding is transparent. The blue and charcoal palette follows the current application CSS.

`size-preview.png` compares the candidates on light and dark surfaces at large and taskbar sizes. Enlarged 16px examples use nearest-neighbor scaling so the remaining pixels can be inspected. Preview resizing does not alter the candidate sources.

`selected-size-preview.png` shows the selected artwork using the actual generated ICO entries at 16px, 32px, and 64px, with the small entries enlarged for inspection.

## Scope

Candidate A supplies the canonical 1024×1024 source at [`src-tauri/icons/source.png`](../../../../src-tauri/icons/source.png). The complete desktop icon set is regenerated from it with `pnpm icons:generate`. Candidates B and C remain as the original alternatives. Windows installer and installed-app verification must use the release-equivalent bundle and existing installer checks in `docs/testing.md`.

These candidates use no external reference assets. The artwork is licensed under the repository's GPL-3.0-or-later license.

## Generation prompts

Each candidate was generated independently with `transparent_background: true`.

### a-modular-g

```text
Use case: logo-brand.
Asset type: original Windows desktop application icon candidate for GMM, Gacha Mod Manager.
Primary request: generate ONE finished square 1024x1024 icon, not a sheet or presentation.
GMM organizes a central Library of mods and connects selected mods to six games. It is an independent desktop utility.
Style/medium: exceptionally simple, precise vector-like flat geometric artwork, suitable for a desktop taskbar. Strong distinct silhouette, thick forms and large negative spaces. Clean contemporary utility design.
Color palette: use the application's existing blue accent #3b82f6, lighter blue #93c5fd, off-white #e5e5e5, and dark charcoal #18181b. No other colors.
Composition/framing: one centered icon with generous transparent padding; the solid charcoal rounded-square tile occupies about 84% of the square canvas. Inside the tile the symbol must be bold and fill about 70% of the tile. Transparent pixels outside the tile. Front view.
Constraints: the central mark must remain recognizable when downscaled to 16x16 and 32x32 pixels; make its strokes very thick and its empty gaps wide. Maximum three simple component shapes. Original geometry only.
Avoid: game artwork, anime, characters, game logos, game trade dress, Tauri branding, XXMI or 3dmigoto branding, affiliation symbols, tiny details, borders, glow, shadows, gradients, textures, three-dimensional rendering, extruded artwork, thin lines, decorative stars, sparkles, captions, wordmarks, watermark, mockups, multiple icons.
Subject: a distinctive single uppercase G emblem, designed from two thick blue modular shapes. The G has an angular rounded-square outer form, an unmistakable broad opening on its right and a short inward crossbar. Use one bright blue main body and one pale blue crossbar module with a generous separating gap, suggesting enabled mod modules coming together. It must read as a clear G rather than a C, a chain, or a maze. No other lettering.
```

### b-library-layers

```text
Use case: logo-brand.
Asset type: original Windows desktop application icon candidate for GMM, Gacha Mod Manager.
Primary request: generate ONE finished square 1024x1024 icon, not a sheet or presentation.
GMM organizes a central Library of mods and connects selected mods to six games. It is an independent desktop utility.
Style/medium: exceptionally simple, precise vector-like flat geometric artwork, suitable for a desktop taskbar. Strong distinct silhouette, thick forms and large negative spaces. Clean contemporary utility design.
Color palette: use the application's existing blue accent #3b82f6, lighter blue #93c5fd, off-white #e5e5e5, and dark charcoal #18181b. No other colors.
Composition/framing: one centered icon with generous transparent padding; the solid charcoal rounded-square tile occupies about 84% of the square canvas. Inside the tile the symbol must be bold and fill about 70% of the tile. Transparent pixels outside the tile. Front view.
Constraints: the central mark must remain recognizable when downscaled to 16x16 and 32x32 pixels; make its strokes very thick and its empty gaps wide. Maximum three simple component shapes. Original geometry only.
Avoid: game artwork, anime, characters, game logos, game trade dress, Tauri branding, XXMI or 3dmigoto branding, affiliation symbols, tiny details, borders, glow, shadows, gradients, textures, three-dimensional rendering, extruded artwork, thin lines, decorative stars, sparkles, captions, wordmarks, watermark, mockups, multiple icons.
Subject: three bold horizontally stacked mod-library cards viewed directly from the front. Each card is a very broad thick rounded bar with one simple offset step; the slightly staggered stack forms a compact organized-library silhouette. Top card off-white, middle card pale blue, bottom card bright blue. Use large clear charcoal gaps between the three cards, no fine strokes and no perspective. No letters or text.
```

### c-connected-modules

```text
Use case: logo-brand.
Asset type: original Windows desktop application icon candidate for GMM, Gacha Mod Manager.
Primary request: generate ONE finished square 1024x1024 icon, not a sheet or presentation.
GMM organizes a central Library of mods and connects selected mods to six games. It is an independent desktop utility.
Style/medium: exceptionally simple, precise vector-like flat geometric artwork, suitable for a desktop taskbar. Strong distinct silhouette, thick forms and large negative spaces. Clean contemporary utility design.
Color palette: use the application's existing blue accent #3b82f6, lighter blue #93c5fd, off-white #e5e5e5, and dark charcoal #18181b. No other colors.
Composition/framing: one centered icon with generous transparent padding; the solid charcoal rounded-square tile occupies about 84% of the square canvas. Inside the tile the symbol must be bold and fill about 70% of the tile. Transparent pixels outside the tile. Front view.
Constraints: the central mark must remain recognizable when downscaled to 16x16 and 32x32 pixels; make its strokes very thick and its empty gaps wide. Maximum three simple component shapes. Original geometry only.
Avoid: game artwork, anime, characters, game logos, game trade dress, Tauri branding, XXMI or 3dmigoto branding, affiliation symbols, tiny details, borders, glow, shadows, gradients, textures, three-dimensional rendering, extruded artwork, thin lines, decorative stars, sparkles, captions, wordmarks, watermark, mockups, multiple icons.
Subject: two large chunky interlocking modules whose combined silhouette forms an unmistakable geometric uppercase M. Two broad arch-like squared bridge pieces fit into one unified emblem through a central notch, symbolizing mods connected to a game. Left module bright blue and right module pale blue. One broad charcoal separation gap makes the two pieces visible without breaking the overall M silhouette. No thin link connectors, no other letters or text.
```
