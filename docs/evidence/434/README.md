# Native dark surfaces — Windows evidence for #434

Collected 2026-11-04 on Windows 11 Home 25H2, OS build 26200.9711.
The baseline is unmodified main at `b1af4af`; the changed build adds the native
Dark preference in this PR. Both are real x64 MSVC Tauri desktop debug builds,
driven through the native UI with the same unchanged frontend. The `Inspect`
entry in their WebView2 menus belongs to the debug build.

## Observed outcomes

| Surface | Outcome under Windows Light | Windows Dark, fresh launch | Evidence |
| --- | --- | --- | --- |
| Title bar and menu bar | **Fixed.** Both change from light to dark. | Both dark; labels and window controls readable. | [Before](before-light-frame.png), [after](after-light-frame.png), [Dark recheck](after-dark-frame.png) |
| File dropdown | **Not controllable through the pinned supported APIs.** It remains light even after Tauri's application and menu-bar preferences are Dark. | Stock dropdown is dark and readable. | [Before](before-light-file-menu.png), [after](after-light-file-menu.png), [Dark recheck](after-dark-file-menu.png) |
| WebView2 context menu | **Fixed.** It changes from white to dark. | Dark and readable. | [Before](before-light-webview-context-menu.png), [after](after-light-webview-context-menu.png), [Dark recheck](after-dark-webview-context-menu.png) |
| GMM-owned Select Folder dialog | **Not controllable through the pinned supported APIs.** A dark owner does not force this common dialog dark; it remains light. | The same picker is dark and readable. | [Before](before-light-folder-dialog.png), [after](after-light-folder-dialog.png), [Dark recheck](after-dark-folder-dialog.png) |

Windows mode: [Light selected](windows-light.png),
[Dark restored](windows-dark-restored.png).

All four surfaces were inspected directly and captured before and after the
change under Windows Light. The changed app was then relaunched with Windows
Dark and all four were inspected and captured again. Every folder picker was
cancelled without selecting a path. Automated tests do not establish these
visual results; the screenshots do.

## Startup policy and API boundaries

The policy is **startup-applied fixed Dark**, the startup option from #434.
It does not sample the OS theme for the controlled surfaces and adds no live
OS-theme-following handler. This matches the single dark palette decided in
#378; #382 and the `prefers-color-scheme` guard are unchanged. Relaunch GMM
after changing Windows mode when checking the two remaining OS-controlled
surfaces; live following is not claimed.

- The configured window theme reaches tao's per-window dark frame handling
  before the initially hidden main window appears. Tauri runtime-wry also
  creates the WebView2 with that window theme; wry 0.55.1 applies it through
  `ICoreWebView2Profile::SetPreferredColorScheme`. Microsoft's
  [profile documentation](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2profile)
  describes the effect on native context menus.
- Tauri 2.11.2's `App::set_theme` forwards Dark to the attached muda menu.
  It must run after `app.set_menu` and before `restore_and_track` shows the
  window. muda 0.19.2's Windows `MenuTheme::Dark` custom-paints the menu bar
  (`WM_UAHDRAWMENU`, `WM_UAHDRAWMENUITEM`, and non-client redraw handling);
  the File popup remains a stock Windows menu. The screenshot establishes
  that this exposed theme control does not force the popup dark under Light.
- tauri-plugin-dialog 2.7.1 uses rfd 0.16.0's Win32 common-item-dialog backend.
  That backend creates `IFileDialog` and offers no theme option. The Light
  capture establishes that the owner's Dark preference is insufficient.
  Forcing these last two surfaces through undocumented process-wide Windows
  hooks, owner drawing, or a replacement dialog is outside this fix.

The API descriptions above were checked against the exact Cargo.lock versions:
[Tauri](https://docs.rs/crate/tauri/2.11.2/source/src/app.rs),
[runtime-wry](https://docs.rs/crate/tauri-runtime-wry/2.11.2/source/src/lib.rs),
[wry](https://docs.rs/crate/wry/0.55.1/source/src/webview2/mod.rs),
[muda](https://docs.rs/crate/muda/0.19.2/source/src/platform_impl/windows/mod.rs),
and [rfd](https://docs.rs/crate/rfd/0.16.0/source/src/backend/win_cid/file_dialog/dialog_ffi.rs).
These limits describe GMM's current dependency/API boundary; they do not claim
that every possible Windows implementation is unable to style a popup or
dialog.

## Isolation and privacy

Visual checks used a temporary fresh profile with no Game paths or Mods.
The original profile was preserved and restored after closing the test app;
its database SHA-256 matched before any original-profile relaunch. Windows is
restored to Dark. The installed release was not replaced.

Screenshots are flattened PNGs without metadata. The Windows mode images are
cropped to the selector. Picker address/search fields and the account row are
covered by explicit `<user> / path redacted` boxes. No raw captures, profile
databases, logs, installers, or local absolute paths are included here.

## Verification scope

The recorded command results are in [verification.txt](verification.txt).
Rust builds and tests keep the required `src-tauri/target` directory. Debug
symbols are disabled through `CARGO_PROFILE_DEV_DEBUG=0` and
`CARGO_PROFILE_TEST_DEBUG=0` to limit disk use; debug assertions remain enabled.
The Windows symlink fixtures require an elevated test process on this host.

The configuration guard pins the native Dark preference. The existing frontend
guard continues to reject `prefers-color-scheme` rules. Neither guard is cited
as pixel-level proof.

Unverified: signed/release MSI packaging, installer smoke/lifecycle jobs,
Windows versions other than this 25H2 machine, high-contrast mode, and live OS
theme switching. The Windows-gated Rust tests are distinct from the installer
smoke described in docs/testing.md section 5; the latter was not run against
the user's installed GMM.
