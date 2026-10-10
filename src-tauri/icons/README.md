# GMM application icon

`source.png` is the canonical 1024×1024 RGBA source for the application icon.
The maintainer selected candidate A, the blue modular G, for issue #273.
The artwork is licensed under GPL-3.0-or-later, like the rest of GMM.

The original was created with the built-in image generation tool, without
external reference assets. Its exact prompt and the candidate comparison are
in [`docs/design/icon-candidates/issue-273/`](../../docs/design/icon-candidates/issue-273/).
The selected original was resized to 1024×1024 with its transparency preserved.

From the repository root, regenerate the desktop icons with the pinned Tauri CLI:

```sh
pnpm icons:generate
```

Tauri regenerates `icon.png`, the PNG sizes, `icon.ico`, `icon.icns`, and the
Square/Store logo set. The command can also produce Android and iOS directories;
GMM ships a Windows desktop bundle and does not use those mobile outputs.

Keep `source.png` when regenerating. Inspect the 16px and 32px entries in
`icon.ico` before accepting an artwork change. The Windows release-equivalent
bundle and installer checks are documented in [`docs/testing.md`](../../docs/testing.md).
