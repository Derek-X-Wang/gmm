# Rootable open-source Android substrates vs. the HSR PC client

Research for issue #91. Re-examines the conclusion of issue #83 (`docs/research/capture-input-substrates.md` on branch `research/capture-input-substrates`) against the axis that survey never weighed: **open-source, rootable** Android substrates, and what root actually buys.

## TL;DR

**Issue #83's conclusion stands — drive the PC client for v1 — but it now stands for a materially different and stronger reason than the one #83 gave, and one of #83's supporting arguments is superseded.**

The dev's pushback is **half right, and the half that is right matters**. Root genuinely does dissolve the Win32 focus-gating problem that issue #90 exists to solve, and it dissolves it in a way no Windows-side workaround cleanly matches. #83 never weighed this and should have. That is a real gap, correctly identified.

But the branch dies on a different rock than #83 predicted, and the rock is load-bearing: **every rootable open-source substrate is Linux-hosted, and on a Windows box none of them reach the GPU.** Not "reaches it slowly" — the stock Microsoft WSL2 kernel does not ship the kernel features any of them need to *boot*, let alone render:

- Microsoft's own shipped WSL2 kernel config has `# CONFIG_KVM is not set` and contains no `CONFIG_ANDROID_BINDER_IPC`, `CONFIG_ANDROID_BINDERFS`, or `CONFIG_ASHMEM` entries at all ([microsoft/WSL2-Linux-Kernel `Microsoft/config-wsl`](https://github.com/microsoft/WSL2-Linux-Kernel/blob/master/Microsoft/config-wsl)). Cuttlefish needs KVM; Waydroid and redroid need binder/binderfs/ashmem. **All three require the end user to hand-compile and swap in a custom WSL2 kernel before the substrate will start.** redroid's own docs walk through exactly this ([redroid-doc `deploy/wsl.md`](https://github.com/remote-android/redroid-doc/blob/master/deploy/wsl.md)); Waydroid has no WSL documentation at all, and a user issue reporting `modprobe: FATAL: Module binder_linux not found ... 5.10.102.1-microsoft-standard-WSL2` was closed **not planned** ([waydroid#712](https://github.com/waydroid/waydroid/issues/712)).
- Even granting a custom kernel, **there is no primary-source evidence anywhere — Microsoft, Mesa, Waydroid, or redroid — of a GPU-accelerated Android container running inside WSL2 at all**, let alone at playable frame rates. redroid's own WSL guide stops at boot and contains no GPU-mode guidance, no performance figures, and no `gpu_mode` instructions. What the projects' own trackers do contain is unresolved failure reports ([redroid#899](https://github.com/remote-android/redroid-doc/issues/899): 250+ second boots with repeated `init: Exec service is hung?` on a correctly custom-built 6.6 kernel, no maintainer resolution).

This is a consumer-distributed Windows desktop app. "Compile your own Linux kernel" is not a setup step GMM can ship, automate, or support.

Two further findings independently weaken the branch even if the GPU path were solved:

- **ARM translation is mandatory and is scoped by Google itself as a dev tool.** Unity's current docs (6000.4) no longer offer generic x86/x86_64 as an Android target architecture at all — only ARMv7, ARM64, and a Magic Leap 2-specific x86-64 option ([Unity Manual, Android Player settings](https://docs.unity3d.com/6000.4/Documentation/Manual/class-PlayerSettingsAndroid.html)). HSR ships ARM-only native code; x86 hosting *requires* translation. Google's own blog says its ARM translation "can only be used for **application development and debug purposes**" ([Android Developers Blog](https://android-developers.googleblog.com/2020/03/run-arm-apps-on-android-emulator.html)), and Google's own emulator release notes document ARM64 translation bugs ([Emulator release notes](https://developer.android.com/studio/releases/emulator)).
- **The one substrate with concrete HSR field evidence shows it crashing.** Three independent reports on Waydroid's own tracker describe HSR closing itself within seconds of launch or at "checking for update" ([#2088](https://github.com/waydroid/waydroid/issues/2088), [#2081](https://github.com/waydroid/waydroid/issues/2081), [#2074](https://github.com/waydroid/waydroid/issues/2074)). **Cause is undiagnosed** — no maintainer logcat analysis, and it is equally consistent with a graphics/ARM-translation bug as with detection. This is not evidence of a block; it is evidence of *unreliability*, which for this purpose is nearly as disqualifying.

**Where #83 was wrong, and it should be corrected in the record:** #83 proposed a virtual HID / kernel-mode input driver on the PC client as the fix for the unattended phase. That remains a candidate, but #83 presented it as roughly equivalent to the emulator's focus-independence. It is not. A virtual HID driver still injects into the host's single systemwide focus stream — it removes the *API* restriction, not the *resource contention*: the game still has to be the foreground window on the one desktop the user also wants to use. An Android guest on its own virtual display does not consume that resource at all. That is a real architectural difference #83 glossed.

**Recommendation: PC client for v1 (unchanged). For the unattended phase, the rootable-Android branch is viable only if the "same Windows PC" constraint is deliberately reopened in favour of a native-Linux host — and if that happens, the right candidate is Cuttlefish or redroid on bare Linux, not anything under WSL2.** Under the standing "same Windows PC" constraint, this branch is closed on the GPU/kernel evidence, not on preference.

---

## Comparison table

| Substrate | Root available? | Host requirement | GPU path on a Windows box | ARM64/IL2CPP viability | License | Maintenance (as of 2026-08-15) | Detection risk | Distribution burden for GMM |
|---|---|---|---|---|---|---|---|---|
| **PC client + WGC + SendInput** (baseline, #83's pick) | N/A | Windows, native | Native D3D, no translation | N/A — native Windows x64 build, no Android involved | N/A (user's own game) | N/A | Pre-existing baseline only; GMM already injects 3dmigoto (ADR 0001) | **Zero.** `src-tauri/src/core/detect/star_rail.rs` already detects it; GMM already spawns the process (`CONTEXT.md`, "Game Session") |
| **Waydroid** | Container root: `waydroid shell` attaches via `lxc-attach ... --clear-env` with no `--uid`, i.e. UID 0 by default ([`tools/helpers/lxc.py`](https://github.com/waydroid/waydroid/blob/main/tools/helpers/lxc.py)). App-level `su` requires third-party [waydroid_script](https://github.com/casualsnek/waydroid_script) — no first-party Magisk integration ([docs](https://docs.waydro.id/faq/community-projects-we-like)) | **Linux only.** Requires Wayland session + systemd + binder/ashmem. No official WSL docs; WSL issue closed *not planned* ([#712](https://github.com/waydroid/waydroid/issues/712)) | **Broken/unproven.** Stock WSL2 kernel lacks binder — won't boot without custom kernel. No primary evidence of GPU-accelerated operation in WSL2 anywhere. Native Linux path uses AOSP Mesa passthrough (Intel/AMD OK; **"For Nvidia GPUs (except tegra) and VMs, we recommend using software-rendering"** — [waydro.id](https://waydro.id/)) | Requires third-party ARM translation (libhoudini/libndk). **Concrete HSR crash reports on this substrate**, cause undiagnosed | GPLv3, dual-licensed with a commercial option ([LICENSE](https://raw.githubusercontent.com/waydroid/waydroid/main/LICENSE), [licensing](https://waydro.id/licensing)) — GPL branch compatible with GMM | **Active.** v1.6.3 (May 2026); last `main` commit 2026-07-24 ([commits](https://github.com/waydroid/waydroid/commits/main)) | Unresolved. Crash reports exist, no diagnosis | **Prohibitive.** Custom kernel compile + Wayland + systemd on a Windows host |
| **redroid** | Yes, explicitly for debug: set `ro.secure=0` → "root adb shell provided by default" ([README](https://github.com/remote-android/redroid-doc/blob/master/README.md)) | Linux + Docker; needs binder/binderfs/ashmem. **Has an official WSL guide — but it is a custom-kernel-build guide** ([`deploy/wsl.md`](https://github.com/remote-android/redroid-doc/blob/master/deploy/wsl.md)) | **Unproven.** `redroid_gpu_mode=host` exists for native Linux + `--privileged`; the official WSL doc contains **no GPU guidance and no performance figures**. [#899](https://github.com/remote-android/redroid-doc/issues/899): 250s+ boots on a correct custom WSL2 kernel, unresolved | Requires ARM translation; no HSR-specific evidence found either way | Claimed Apache-2.0 (kernel modules GPL-2.0) **in README prose only — no LICENSE file exists in the repo** ([contents API](https://api.github.com/repos/remote-android/redroid-doc/contents/)) | **Active-ish.** Last `master` commit 2026-05-17 — ~3 months stale | Unresolved; no HSR data | **Prohibitive.** Custom kernel + Docker + privileged container |
| **Cuttlefish** (AOSP) | **Yes, first-class.** `adb root` is part of the documented standard workflow ([get-started](https://source.android.com/docs/devices/cuttlefish/get-started)); images are userdebug/eng | **Linux + KVM only.** Docs specify "Linux x86 and ARM64 machines" and a `/dev/kvm` check. **No mention of Windows or WSL2 anywhere** ([docs](https://source.android.com/docs/devices/cuttlefish)) | **Blocked on WSL2.** Stock WSL2 kernel has `# CONFIG_KVM is not set`. On native Linux, gfxstream forwards GL/Vulkan to host GPU ([GPU doc](https://source.android.com/docs/devices/cuttlefish/gpu)) — the cleanest GPU story of the three, on the right host | Google's ARM translation is Play/Google-APIs-image-only and **"development and debug purposes" only** per Google | **Apache 2.0** ([LICENSE](https://github.com/google/android-cuttlefish/blob/main/LICENSE)) — cleanest license of the set | **Most active.** Daily commits through Aug 2026; live `android16-*` branches ([refs](https://android.googlesource.com/device/google/cuttlefish/+refs)) | Highest structural risk — it is a *developer* virtual device with no vendor anti-detection engineering, and AOSP images ship no GMS by default | **Prohibitive on Windows.** Best candidate *if* a native-Linux host is accepted |
| **Genymotion** | Yes (`adb root`, `persist.sys.root_access` — [docs](https://docs.genymotion.com/features/root/)) | Windows/Linux/macOS via VirtualBox | Reaches GPU via VirtualBox on Windows | Requires ARM translation | **Proprietary/commercial — no OSS edition** ([genymotion.com](https://www.genymotion.com/)) | Active, commercial | Unknown | **DISQUALIFIED** on the open-source criterion this ticket set |
| **Anbox** | (moot) | Linux | (moot) | (moot) | GPLv3 | **DEAD.** Repo archived 2024-02-13; README: development "has stalled" and "it's no longer actively developed" ([repo](https://github.com/anbox/anbox)). Canonical's *Anbox Cloud* is a separate commercial product | (moot) | **DISQUALIFIED** — unmaintained |
| **Android-x86** | Not confirmed from a primary source (community reports of `su` in `/system/xbin`; **no android-x86.org statement found**) | Bare metal or a full hypervisor VM (VirtualBox/QEMU/Hyper-V). Not a WSL2 path | Via the hypervisor; a separate VM stack from WSL2 | Historically bundled houdini | Apache 2.0 + some GPL-2.0 ([android-x86.org](https://www.android-x86.org/)) | **Stalled.** Latest stable 8.1-r6 (2021); 9.0-r2 dated 2020-03-25; `r-x86` branch "ready for developers" Apr 2022, no release since ([changelog](https://www.android-x86.org/changelog.html)) | Unknown | Prohibitive; and effectively abandoned |
| **Bliss OS** | KernelSU on alpha builds ([SourceForge](https://sourceforge.net/projects/blissos-dev/files/Android-Generic/PC/bliss/S/foss/alpha/)); note open upstream KernelSU reliability bug [tiann/KernelSU#2113](https://github.com/tiann/KernelSU/issues/2113) | Bare metal or hypervisor VM; not WSL2 | Via the hypervisor | Bundles ARM translation libraries (proprietary redistributables) | Mixed Apache-2.0/GPL-2.0/GPL-3.0, **plus proprietary redistributables (firmware, codecs, ARM translation, Widevine L3) whose commercial redistribution is explicitly prohibited without separate licensing** ([licensing](https://github.com/BlissRoms-x86/website/blob/master/licensing.html)) | **Active.** 2026 commit activity ([BlissOS org](https://github.com/BlissOS)) | Unknown | Prohibitive; licensing of the bundled blobs is a further hazard for a GPL-3.0 app |

---

## Thread 1: What root actually does to focus gating

This is the load-bearing thread and the answer has two distinct halves that must not be conflated.

### Half 1 — host-level gating: root dissolves it, because the concept doesn't exist at that layer

The Linux kernel's own uinput documentation describes the mechanism with **no mention of focus, foreground, or targeting anywhere**: "By writing to /dev/uinput ... a process can create a virtual input device with specific capabilities. Once this virtual device is created, the process can send events through it, that will be delivered to userspace and in-kernel consumers" ([kernel.org, `Documentation/input/uinput.rst`](https://www.kernel.org/doc/Documentation/input/uinput.rst)).

This is not an omission — focus is genuinely not a concept at the evdev/uinput layer. Android's own architecture docs confirm where focus *does* enter: kernel driver → `EventHub` → `InputReader` → `InputDispatcher`, and it is the dispatcher that "forwards them to the appropriate window" ([source.android.com, Input](https://source.android.com/docs/core/interaction/input)). Injection happens several layers *below* any focus decision.

Contrast with Win32, which #83 pinned correctly: `SendInput` has no window-handle parameter by design, and "the system posts keyboard messages to the message queue of the foreground thread that created the window with the keyboard focus" ([Keyboard Input Overview](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-keyboard-input)). Focus gating is an explicit architectural property of the Win32 input model. It is an explicit *non*-property of the Linux input model.

**So: yes, root-level `/dev/uinput` injection is genuinely not host-focus-gated. Confirmed from primary sources. #90's premise, as stated, does not survive contact with this substrate.**

### Half 2 — guest-side visibility: root does NOT dissolve this

Injected events still traverse Android's own `InputDispatcher`, which applies its own rules. From AOSP source (primary — this *is* the official implementation):

- **Key events** require a focused window: "If there is no currently focused window and no focused application then drop the event." ([`InputDispatcher.cpp`, main](https://android.googlesource.com/platform/frameworks/native/+/refs/heads/main/services/inputflinger/dispatcher/InputDispatcher.cpp))
- **Touch events** are hit-tested by geometry rather than keyboard focus, but the window must be visible: `if (windowInfo.displayId != displayId || inputConfig.test(WindowInfo::InputConfig::NOT_VISIBLE)) { return false; }` (same file). This invariant is long-standing — the equivalent `if (windowInfo->visible)` check appears in [android-4.2.1_r1](https://android.googlesource.com/platform/frameworks/base/+/android-4.2.1_r1/services/input/InputDispatcher.cpp).
- Useful nuance: a *visible but partially obscured* window still receives touch, merely tagged `WINDOW_IS_OBSCURED`. Only genuinely invisible/backgrounded windows are excluded.

`adb shell input tap` is not an exception — it constructs `MotionEvent`s and calls `InputManager.injectInputEvent()`, feeding the same dispatcher ([`Input.java`](https://android.googlesource.com/platform/frameworks/base/+/android-4.4.2_r1/cmds/input/src/com/android/commands/input/Input.java)). *(Note: there is no official developer.android.com reference page for the `input` shell command at all; the AOSP source is the primary source here.)*

### What this actually means for #90 — the precise answer

**The two constraints are different resources, and that is the whole point.**

- Windows: the game must hold **the single systemwide focus of the machine the user is sitting at**. Satisfying it costs the user their PC.
- Android guest: the game must be **the visible foreground activity on the Android guest's own virtual display**. Satisfying it costs nothing the user wanted — that display is not the user's desktop.

So root does not remove a foreground requirement; it **relocates the foreground requirement onto a display nobody is using.** That is a genuine architectural win and #83 did not account for it.

It also explains why #83's proposed fix is not equivalent. A virtual HID / kernel-mode input driver removes the *API-level* restriction on the Windows side, but the injected events still land in the host's one focus stream and the game still needs to be the foreground window of the user's actual desktop. **The driver solves the API problem; it does not solve the resource-contention problem.** #83 treated these as the same. They are not, and that correction belongs in the record regardless of which substrate wins.

**Verdict on #90's premise: it does not survive root — but root is only reachable at a cost the rest of this document prices, and the benefit is entirely a phase-2 (unattended) benefit. Attended v1 does not care, because during attended v1 the user is present and the window is legitimately foreground anyway.**

---

## Thread 2: Per-candidate detail

### Waydroid

Runs "a full Android system in a container" using Linux namespaces, with "direct access to needed hardware through LXC and the binder interface" ([waydro.id](https://waydro.id/)). Ships a LineageOS-based image (Android 13 current, Android 16 images in progress per the 1.6.3 changelog).

**Root.** Two different things get called "root" here and the distinction matters. `waydroid shell` gives *container* root — the source builds `lxc-attach -P <path> -n waydroid --clear-env` and only appends `--uid`/`--gid` when a caller explicitly passes them, so the default attach is UID 0 ([`tools/helpers/lxc.py`](https://github.com/waydroid/waydroid/blob/main/tools/helpers/lxc.py)). That is enough for `/dev/uinput` injection and `/data` access. Granting `su` to Android *apps* is a separate matter with no first-party support; the official docs point at a community tool ([Community Projects We Like](https://docs.waydro.id/faq/community-projects-we-like)).

**GPU.** "Waydroid uses Android's mesa integration for passthrough ... Intel/AMD GPUs for the PC side. For Nvidia GPUs (except tegra) **and VMs, we recommend using software-rendering**" ([waydro.id](https://waydro.id/)). That parenthetical is not a footnote for this ticket — a WSL2-hosted Waydroid *is* the VM case, and Waydroid's own recommendation for it is software rendering. Software rendering a Unity 3D title is not a real option.

**Host.** Requires a Wayland session ("X11 sessions are not supported"), systemd, and binder/ashmem. None are WSL2 defaults; binder is absent from the stock kernel entirely ([#712](https://github.com/waydroid/waydroid/issues/712), closed *not planned*).

**License.** GPLv3, with a commercial dual-license option offered for proprietary use ([LICENSE](https://raw.githubusercontent.com/waydroid/waydroid/main/LICENSE), [licensing page](https://waydro.id/licensing)). The GPLv3 branch is compatible with GMM's own GPL-3.0-or-later posture (ADR 0001) — licensing is *not* a blocker here.

**Maintenance.** Healthy: v1.6.3 released May 2026, last `main` commit 2026-07-24.

**HSR specifically.** The only substrate in this survey with direct HSR field reports, and they are bad: crashes within seconds of launch or at "checking for update" ([#2088](https://github.com/waydroid/waydroid/issues/2088), [#2081](https://github.com/waydroid/waydroid/issues/2081), [#2074](https://github.com/waydroid/waydroid/issues/2074)), all on Steam Deck / ROG Ally hardware, none with a maintainer diagnosis. One reporter speculates about "security"; that speculation is unverified and I am not treating it as evidence of detection. Countervailing reports suggest some users get it running with ARM translation + GApps enabled. **Read this as: inconsistent and unreliable, cause unknown.**

### redroid

Containerized Android in Docker — "You can boot many instances in Linux host (Docker, podman, k8s etc.)," covering Android 8.1–16 ([README](https://github.com/remote-android/redroid-doc/blob/master/README.md)).

**Root.** `ro.secure=0` yields "root adb shell provided by default," explicitly flagged as for DEBUG purposes (same README). Clean and documented.

**GPU.** `androidboot.redroid_gpu_mode` takes `auto`/`host`/`guest`, with `guest` (software) as the default and `host` for passthrough; standard invocation needs `--privileged`. On native Linux this is a real path. **In the official WSL guide it is simply absent** — [`deploy/wsl.md`](https://github.com/remote-android/redroid-doc/blob/master/deploy/wsl.md) covers building a kernel with `CONFIG_ANDROID_BINDER_IPC`, `CONFIG_ANDROID_BINDERFS`, `CONFIG_DMABUF_HEAPS`, `CONFIG_ASHMEM` and pointing `.wslconfig` at the resulting `bzImage`, and stops there. No GPU mode, no numbers.

**Host.** Kernel 4.14+ with binder/ashmem modules, or 5.0+ with binderfs/ashmem compiled in ([redroid-modules README](https://github.com/remote-android/redroid-modules/blob/master/README.md)). Stock WSL2 has none of it. Even with a correct custom build, [#899](https://github.com/remote-android/redroid-doc/issues/899) reports 250s+ boots with repeated `init: Exec service is hung?`, unresolved. A separate open issue ([#859](https://github.com/remote-android/redroid-doc/issues/859)) shows current images still look for legacy `/dev/binder` rather than binderfs, breaking on modern kernels.

**License.** README prose claims Apache-2.0 for redroid and GPL-2.0 for the kernel modules, and warns "redroid includes many 3rd party modules, you may need to examine license carefully." **There is no LICENSE file in the repository** ([contents API](https://api.github.com/repos/remote-android/redroid-doc/contents/)). For a GPL-3.0 project that has already thought carefully about license propagation (ADR 0001), depending on a substrate whose licensing exists only as a README sentence is a real, if secondary, hazard.

**Maintenance.** Last `master` commit 2026-05-17 — roughly three months stale.

### Cuttlefish (AOSP)

Google's own virtual device, "the canonical device for representing the current state of AOSP development" with "full fidelity with Android Framework" ([source.android.com](https://source.android.com/docs/devices/cuttlefish)).

**Root.** First-class and documented: `adb root` appears in the standard update workflow (`adb root` → `adb remount -R` → `adb sync` → `adb reboot`) ([get-started](https://source.android.com/docs/devices/cuttlefish/get-started)), with userdebug images offered directly from CI. This is the cleanest root story of the set — no third-party tooling, no debug-prop hacks.

**Host.** "dependent on virtualization being available on the host machine" (KVM; check `/proc/cpuinfo` for `vmx|svm` or `/dev/kvm`), supported on "Linux x86 and ARM64 machines." **Windows and WSL2 appear nowhere in the official documentation.** And the stock WSL2 kernel ships `# CONFIG_KVM is not set` — the one thing Cuttlefish cannot do without.

**GPU.** Three modes: `--gpu_mode=gfxstream` (forwards GL/Vulkan to host), `--gpu_mode=drm_virgl` (Virgl, no Vulkan), SwiftShader (software) ([GPU doc](https://source.android.com/docs/devices/cuttlefish/gpu)). gfxstream on a native Linux host with proper EGL/GLES/Vulkan drivers is the strongest GPU story among all candidates — on the *right host*.

**License.** Apache 2.0 ([LICENSE](https://github.com/google/android-cuttlefish/blob/main/LICENSE)); gfxstream is Apache-2.0 primary with some MIT/BSD/ISC components ([LICENSE](https://android.googlesource.com/platform/hardware/google/gfxstream/+/refs/heads/main/LICENSE)). Cleanest licensing of the set.

**Maintenance.** The most active candidate — daily commits through August 2026 on [google/android-cuttlefish](https://github.com/google/android-cuttlefish), live `android15-`/`android16-*` release branches.

**Caveat that cuts the other way.** Cuttlefish is a *development* device. AOSP images carry no GMS, and it has zero vendor anti-detection engineering. If HSR does any integrity checking at all, this is the substrate most exposed to it — precisely because it is the most honestly what it is.

### Genymotion — disqualified

No open-source edition exists; the product ships as commercial Desktop/SaaS/PaaS editions only ([genymotion.com](https://www.genymotion.com/)). It does support root (`adb root`, `persist.sys.root_access` — [docs](https://docs.genymotion.com/features/root/)), so it is capable, but it fails the open-source criterion this ticket set. Noted and excluded.

### Anbox — dead

Repository archived 2024-02-13, entire GitHub org archived. README states development "has stalled in the past years," "it's no longer actively developed," and "the existing repositories will remain as is but no active maintenance will be applied going forward" ([anbox/anbox](https://github.com/anbox/anbox)). GPLv3. Canonical's **Anbox Cloud** is a separate, diverged, commercial product ([docs](https://documentation.ubuntu.com/anbox-cloud)) and is out of scope on the open-source criterion regardless. **Excluded.**

### Android-x86 — stalled

Latest stable is 8.1-r6 (2021); the Pie-based 9.0-r2 dates to 2020-03-25; an April 2022 note declares the Android-11-based `r-x86` branch "ready for developers" with no stable release since ([changelog](https://www.android-x86.org/changelog.html)). Apache 2.0 with some GPL-2.0 components ([android-x86.org](https://www.android-x86.org/)). **Root-by-default could not be confirmed from any primary android-x86.org page** — community sources describe `su` in `/system/xbin`, but I am flagging that as unverified rather than citing it. Architecturally it is a bare-metal/VM x86 Android distro, so hosting it on Windows means VirtualBox/VMware/Hyper-V — a full second hypervisor stack, not a WSL2 path. Effectively abandoned; excluded on maintenance.

### Bliss OS

The most actively maintained of the x86 Android distros (2026 commit activity across [BlissOS](https://github.com/BlissOS)), with root via KernelSU on alpha builds ([SourceForge](https://sourceforge.net/projects/blissos-dev/files/Android-Generic/PC/bliss/S/foss/alpha/)) — though there is an open upstream bug where KernelSU reports root as granted while `su` calls fail for some apps ([tiann/KernelSU#2113](https://github.com/tiann/KernelSU/issues/2113)).

Its licensing is the messiest of the set and matters for a GPL-3.0 app: Apache-2.0 inherited from AOSP, GPL-2.0 for Android-Generic components, **plus proprietary redistributables bundled into public builds — firmware blobs, proprietary drivers, media codecs, ARM translation libraries, Widevine L3 — with commercial redistribution of GApps-inclusive builds explicitly prohibited without separate licensing** ([licensing page](https://github.com/BlissRoms-x86/website/blob/master/licensing.html)). GMM could at most detect it, never bundle it. Same hypervisor-on-Windows hosting problem as Android-x86.

---

## Thread 3: GPU acceleration under WSL2 — the load-bearing risk

The path is real, and I want to be fair to it before saying why it fails.

**It exists and it is documented.** `/dev/dxg` is a genuine kernel driver exposing "a set of IOCTL that closely mimic the native WDDM D3DKMT kernel service layer on Windows," over "WDDM GPU Paravirtualization, or GPU-PV," which "connects over the VM Bus to its big brother on the Windows host" ([DirectX ❤ Linux](https://devblogs.microsoft.com/directx/directx-heart-linux/)). It is not WSLg-specific: "`/dev/dxg` is automatically exposed and available to any WSL distro installed without having to install any additional packages" (same). Microsoft's driver docs confirm the architecture — no KMD in the guest, no VidMm/VidSch, thunk calls marshalled to the host over VM bus channels with a 128KB max message size ([GPU paravirtualization](https://learn.microsoft.com/en-us/windows-hardware/drivers/display/gpu-paravirtualization)). On top of it, Mesa's D3D12 Gallium driver "emits API calls for Microsoft's D3D12 API instead of targeting a specific GPU architecture," providing "full desktop OpenGL 3.3 support on devices that only support D3D12" ([docs.mesa3d.org/drivers/d3d12](https://docs.mesa3d.org/drivers/d3d12.html)), with Dozen/Dzn as the Vulkan-on-D3D12 counterpart.

**Now the problems, in ascending order of severity.**

1. **Documented overhead on the graphics path.** Microsoft's own WSLg architecture post: "rendered data is copied from VRAM to system memory before being presented to the compositor ... and uploaded onto the GPU again on the Windows side. As a result, there is a performance penalty proportionate to the presentation rate," benchmarked by Microsoft at roughly **35% throughput loss on an RTX 3090 (350fps vs 540fps native)** ([WSLg Architecture](https://devblogs.microsoft.com/commandline/wslg-architecture/)). To be scrupulous: that specific copy is the Weston/WSLg *compositor* path, and a headless Android container rendering to a virtual display would not necessarily hit that exact bottleneck. But no primary source documents what the non-WSLg case actually costs, and a separate Microsoft-repo thread complaining of 20–50% loss through the vGPU layer was **closed as "not planned" with no engineering rebuttal** ([microsoft/DirectML#166](https://github.com/microsoft/DirectML/issues/166)).

2. **Vulkan maturity is officially unanswered.** An open issue on WSLg's own tracker asking for "Status and Roadmap for Vulkan 1.3 Compliance in D3D12-based Dzn Driver for WSLg" sits **unanswered by maintainers** ([wslg#1340](https://github.com/microsoft/wslg/issues/1340)). Mesa's Venus and Zink docs describe virtio-gpu and Vulkan-backed GL paths respectively, but **neither mentions WSL2 as a target** ([venus](https://docs.mesa3d.org/drivers/venus.html), [zink](https://docs.mesa3d.org/drivers/zink.html)) — Venus targets virtio-gpu transports (crosvm/QEMU), which is a different mechanism from WSL2's Hyper-V GPU-PV entirely.

3. **The substrates don't boot on the stock kernel.** `Microsoft/config-wsl` has `# CONFIG_KVM is not set` and no binder/binderfs/ashmem entries at all ([config-wsl](https://github.com/microsoft/WSL2-Linux-Kernel/blob/master/Microsoft/config-wsl)). KVM exists as compilable source in the tree but is off by default. WSL2 does support Hyper-V *nested virtualization* as a platform feature (`nestedVirtualization` in `.wslconfig` — [wsl-config](https://learn.microsoft.com/en-us/windows/wsl/wsl-config)), but that is a different mechanism and does not give the WSL2 guest in-guest KVM. Every candidate needs a hand-compiled kernel swapped in via `.wslconfig`'s `kernel=` directive before it will start.

4. **The decisive one: nobody has demonstrated it.** Across Microsoft's docs, Mesa's docs, Waydroid's repo, and redroid's repo, **there is no primary-source record of a GPU-accelerated Android container running inside WSL2** — not a benchmark, not a success report, not a maintainer confirmation. The closest official artifact, redroid's own `deploy/wsl.md`, stops at boot and says nothing about GPU. What the trackers contain instead is unresolved failure ([redroid#899](https://github.com/remote-android/redroid-doc/issues/899), [waydroid#712](https://github.com/waydroid/waydroid/issues/712), [waydroid#332](https://github.com/waydroid/waydroid/issues/332)).

**Blunt assessment, as the ticket asked for:** the translation chain a WSL2-hosted Android substrate would need — Android's GLES/Vulkan renderer → gfxstream or a GLES shim → Mesa D3D12 or Dozen → GPU-PV → host D3D12 driver — has never been validated end-to-end in any primary source for a GPU-bound 3D Android workload, sits on top of a virtualization layer Microsoft itself measures at a ~35% penalty on its documented path, and cannot even be attempted without the user compiling a kernel. **For a real-time Unity 3D title this should be treated as non-viable, not merely risky.** Absence of evidence is genuinely the finding here: this combination is exotic enough that nobody with the relevant expertise appears to have made it work and written it down.

---

## Thread 4: ARM64 IL2CPP under x86 Android emulation

**The architectural fact is solid and primary-sourced.** Unity's current Android Player settings (6000.4) list target architectures as **ARMv7, ARM64, and "x86-64 (Magic Leap 2)"** — generic x86/x86_64 is no longer offered for standard Android builds at all ([Unity Manual 6000.4](https://docs.unity3d.com/6000.4/Documentation/Manual/class-PlayerSettingsAndroid.html)). Older versions (2021.3) did offer four separate targets, x86 among them, but as an explicit opt-in ([Unity Manual 2021.3](https://docs.unity3d.com/2021.3/Documentation/Manual/class-PlayerSettingsAndroid.html)). ARM64 "is enabled only when you set Scripting Backend to IL2CPP." Google concurs that x86 native code in Android apps is rare in practice ([Support 64-bit architectures](https://developer.android.com/games/optimize/64-bit)).

**Conclusion: HSR ships ARM-only native code. Any x86 Android substrate must translate. This is not avoidable.**

**Google's own translation layer is scoped as a dev tool.** Google's approach runs the system natively in x86 and translates only the ARM binary "within that process" ([Android Developers Blog](https://android-developers.googleblog.com/2020/03/run-arm-apps-on-android-emulator.html)) — a genuinely smart design. But the same post states it "can only be used for **application development and debug purposes**," and it is restricted to Google APIs/Play Store system images. Google's own emulator release notes document real translation failures: "Some ARMv7 binaries fail to run on Android 11 x86 and x86_64 system images," and a later entry fixing "issues running some ARM64 applications through NDK translation" ([Emulator release notes](https://developer.android.com/studio/releases/emulator)). Faster than full ARM emulation — Google's claim — is a relative statement, not a playability claim.

**libhoudini has no official documentation whatsoever.** I could find no Intel, Google, or Android-x86 primary technical documentation for it. Everything available is reverse-engineering repos and archival mirrors. **I am flagging this rather than citing community sources as authoritative** — it means the translation layer that most consumer emulators actually rely on is, from a due-diligence standpoint, undocumented.

**Playability evidence is weak, and I want to be explicit about how weak.** There is no HoYoverse statement on emulator support either way (the official PC Launcher FAQ is a JS-rendered SPA that would not yield text; the platform-support support article returned HTTP 403). What exists is vendor marketing: BlueStacks' HSR page gives no fps figures; LDPlayer advertises "120 FPS" with no disclosed methodology; MuMu recommends i7-7700 / GTX 1060 6GB / 8GB+ RAM — **notably higher than HSR's own official PC minimums (i3-2120 / GTX 650)**, which is suggestive of substantial emulation overhead but is circumstantial, not causal proof. No independent benchmark and no citable community-consensus thread was located.

**Weight accordingly:** high confidence that translation is mandatory and that Google scopes its own version as dev-only; **low confidence on the "but does it actually play well" question**, which currently rests on self-interested vendor marketing. I am not going to dress that up as stronger than it is — but note that it doesn't need to be strong, because thread 3 already closes the branch on the Windows host.

---

## Thread 5: Root / emulator detection by the HSR Android client

*Scope note: this section reports only whether detection exists and what it implies. Evasion, bypass, spoofing, and hiding techniques are out of scope for this research and are deliberately not covered.*

**Official policy: no explicit root or emulator language found.** I retrieved and read the full HoYoPlay Terms of Service PDF (linked from [hsr.hoyoverse.com/en-us/company/terms](https://hsr.hoyoverse.com/en-us/company/terms)). Its Cheat Detection clause states the games "may contain Cheat Detection software or features," defining Cheats as "programs, methods, processes or other programs with software or hardware on any formats that may give Users an unfair competitive advantage," and warns that removing or disabling Cheat Detection terminates the license ([ToS PDF](https://fastcdn.hoyoverse.com/static-resource-v2/2024/07/01/1ed4cce564ae5dee2f903d37e814b1e0_6707783454148192051.pdf)).

**The words "root," "rooted," "emulator," "virtual environment," and "modified device" do not appear anywhere in that document.** The clause is broad enough that one could argue it reaches such environments, but it does not name them. This is a genuine finding, and it also improves on #83, which could only paraphrase HoYoverse's terms from search-engine synthesis — the Cheat Detection text above is now first-hand.

The HSR Fair Gaming Declaration ([news/111244](https://hsr.hoyoverse.com/en-us/news/111244)) remains unreadable to automated fetch (JS-rendered SPA serving only a loading shell) — same limitation #83 hit. An official HoYoverse ZZZ statement (same publisher) uses the familiar "plug-ins, boosters, and other third-party tools that affect the fairness of the game are strictly prohibited" framing — again naming tools, not device states.

**Play Integrity: capability confirmed, HSR integration not.** Google's own docs confirm the API's `deviceIntegrity` verdict determines whether "the app runs on a genuine certified Android device," with tiered verdicts, and Google recommends tiered enforcement rather than hard blocking ([Play Integrity overview](https://developer.android.com/google/play/integrity/overview)). **No primary source confirms HSR calls it.** #83 inferred this and marked it low-confidence; that assessment holds — it remains unconfirmed rather than established.

**Field evidence on a genuinely rootable substrate.** The three Waydroid HSR crash reports ([#2088](https://github.com/waydroid/waydroid/issues/2088), [#2081](https://github.com/waydroid/waydroid/issues/2081), [#2074](https://github.com/waydroid/waydroid/issues/2074)) are the only concrete data. **Their cause is undiagnosed.** No maintainer logcat analysis exists; one reporter speculates about "security" without evidence; other reports indicate some users get HSR running with ARM translation and GApps. A structurally similar unresolved Genshin black-screen bug on Anbox ([anbox#1624](https://github.com/anbox/anbox/issues/1624)) was attributed by its reporter to rendering, not detection. Inconsistent behaviour across setups points toward a compatibility bug rather than a deliberate binary block.

**Important scoping point the ticket asked about, and it holds.** #83 observed that BlueStacks/MuMu/LDPlayer publish HSR marketing pages, which is decent evidence HSR runs on *those* products. **That evidence does not transfer** to Waydroid/redroid/Cuttlefish. Consumer emulator vendors do first-party engineering to present as ordinary devices; a raw AOSP-derived rootable substrate does none of that. These are architecturally different situations and should not be reasoned about interchangeably.

**Verdict: MIXED / INCONCLUSIVE — this is not a confirmed branch-killer, and I am not going to report it as one.** There is no sourced statement that HSR performs deliberate root/emulation detection-and-block on open rootable substrates. There is suggestive but non-diagnostic evidence of instability. Resolving this properly would need either a maintainer-confirmed Waydroid diagnosis or a teardown of HSR's native libraries — neither of which is worth commissioning while thread 3 keeps the branch closed for unrelated reasons.

*Research gaps, stated honestly:* Reddit was not fetchable by the research tooling, and HoYoLAB threads on this topic ([35348603](https://www.hoyolab.com/article/35348603), [24636977](https://www.hoyolab.com/article/24636977)) are JS-rendered and did not yield body text — and would be user-generated posts, not official statements, even if they had.

---

## Thread 6: The cost side of the ledger, specific to GMM

**Setup burden — the disqualifier for a consumer app.** GMM ships as a Windows desktop mod manager to non-technical gacha players. The minimum viable setup for any rootable substrate on their machine is: install WSL2 → clone `WSL2-Linux-Kernel` → edit a kernel config to enable binder/binderfs/ashmem (or KVM) → install a build toolchain → compile a kernel → edit `.wslconfig` → restart WSL → install Docker or LXC/Wayland/systemd → pull an Android image → configure ARM translation → sideload the HSR APK → log in a second time on a second device fingerprint. **GMM cannot automate a kernel compile on a user's machine, and should not try.** By contrast, the PC-client path needs zero new install-detection work at all — `src-tauri/src/core/detect/star_rail.rs` already finds and validates the exact install, and GMM already holds a process handle during a Game Session (`CONTEXT.md`).

**Packaging and distribution.** GMM's distribution model is a signed Windows installer (see the existing MSI/upgrade-identity work in the repo history). None of these substrates are redistributable inside it: Waydroid needs a Linux userland; redroid needs Docker plus a privileged container; Cuttlefish needs KVM. At absolute best GMM could *detect* an already-working substrate — the same "new detection primitive" problem #83 identified for consumer emulators, except worse, because here liveness detection would have to reach across a WSL2 boundary into a container.

**Licensing.** Genuinely not the blocker, and it deserves saying plainly since ADR 0001 makes GMM license-sensitive. Waydroid's GPLv3 branch and Cuttlefish's Apache-2.0 are both fine alongside GMM's GPL-3.0-or-later. Two real hazards do exist: redroid asserts Apache-2.0 in README prose with **no LICENSE file in the repo**, and Bliss OS bundles proprietary redistributables (ARM translation libs, codecs, Widevine L3) whose redistribution is explicitly restricted. Neither is fatal to *using* the software; both would be fatal to *bundling* it.

**Attack surface.** redroid's documented invocation requires `--privileged`; Waydroid requires root on the host to run `waydroid` at all; all paths require a user-compiled kernel of unverified provenance. GMM currently asks for no elevation for its core junction-based workflow (junctions were chosen over symlinks in ADR 0003 specifically to avoid needing admin rights or Developer Mode). Requiring host root plus a custom kernel is a substantial and philosophically backwards escalation for this codebase.

**Operational duplication.** Unchanged from #83 and still true: automating dailies on an Android substrate means a second multi-GB install, a second login, and a second device fingerprint on the user's HoYoverse account, purely to script menus the PC client can already drive.

**Phase accounting — what pays off when.** The ticket asked for this explicitly, so stating it flatly:

| Capability root buys | Attended v1 | Unattended phase 2 |
|---|---|---|
| Focus-independent input (uinput, no host focus contention) | **No value.** User is present; window is legitimately foreground | **Real value.** This is the genuine win |
| Guest-side capture that survives the host window being minimized | **Marginal.** WGC handles occlusion fine; only true minimization breaks it | **Real value** |
| Snapshots / reproducible state for flow authoring | **Some value** for testing flows | Some value |
| Multiple instances, headless operation | **No value** — v1 is one attended run | Value only for multi-account, which is not a stated goal |
| `/data` reads, Frida attach, memory reading | **No value** — explicitly an optional later capability, and v1 reads the screen | Speculative |

**Nothing in the root column pays off in attended v1.** Every genuine benefit is a phase-2 benefit. Meanwhile every cost — the kernel compile, the GPU risk, the ARM translation risk, the setup burden — is paid in full up front, at v1, before any of the benefits become collectable. That asymmetry is by itself sufficient to settle the v1 question independently of the GPU evidence.

---

## Does issue #83's conclusion stand?

**It stands — with one supporting argument corrected and one gap acknowledged as real.**

**What stands, and is now better supported than it was:**
- PC client for v1 is correct. #83 got the right answer.
- Its "zero new detection primitive" and "GMM already owns the process" arguments are unaffected and, if anything, stronger here: an emulator needed a new ADB liveness probe; a WSL2-hosted container would need that probe *plus* a cross-VM boundary.
- Its HoYoverse-ToS reasoning is now upgraded from paraphrase to first-hand text (the Cheat Detection clause above), and the direction of that finding is unchanged.

**What was a genuine gap, as the ticket alleged:**
- #83 never evaluated open-source rootable substrates and never treated root as a criterion. That was a real omission, and the dev was right to push. This document closes it.

**What #83 got wrong and should be corrected:**
- #83 stated that a virtual HID / kernel-mode input driver "would deliver focus-independent input to the native client too," presenting it as an equivalent substitute for the emulator's focus-independence. **That is not right.** A virtual HID driver removes the API-level restriction but still injects into the host's single systemwide focus stream; the game must still be the foreground window on the user's actual desktop. An Android guest on a virtual display removes the contention entirely. #83 conflated "API allows it" with "the user gets their PC back." They are different, and the difference is exactly what phase 2 cares about.

**What changes about the *reasoning* even though the answer is the same:** #83 rejected emulators on an accumulation of soft factors — detection complexity, EULA uncertainty, capture latency, operational duplication. Those are all still true, but they are not why this branch closes. **This branch closes on a hard technical fact: on a Windows host, no rootable open-source substrate reaches the GPU, and two of the three cannot even boot without the user compiling a kernel.** That is a firmer foundation than #83's, and it is worth having the stronger reason on record.

**Verdict: STANDS, on better evidence, with one of its arguments corrected.**

---

## Does issue #90's premise survive?

#90's premise is that unattended runs need a way past Win32 focus gating, and it lists four candidate resolutions (hold focus, separate session/virtual desktop, dedicated machine, virtual HID driver).

**The premise survives as a problem, and root is a genuine fifth answer to it — but an unreachable one under the current constraint.**

Precisely:

1. **Root does dissolve host-level focus gating.** Confirmed from the kernel's own uinput docs and AOSP's input-pipeline architecture: focus is not a concept at the injection layer. This is not a workaround; the problem structurally does not exist there.
2. **Root does not remove a foreground requirement altogether.** Android's `InputDispatcher` still drops key events with no focused window and still excludes `NOT_VISIBLE` windows from touch targeting (AOSP source, cited above). What changes is *which display* that requirement applies to — the guest's virtual display instead of the user's desktop.
3. **That relocation is the actual win, and it is real.** "The game must be foreground somewhere the user isn't looking" is a completely different proposition from "the game must own the machine the user is sitting at."
4. **It is unreachable on a Windows host** for the GPU/kernel reasons in thread 3.
5. **It costs nothing to defer.** #90 explicitly allows "deliberately deferred, and here is why that is safe" as a legitimate resolution. It is safe here: nothing in an attended v1 flow model commits to an input mechanism, so keeping the flow layer substrate-agnostic preserves every option including this one.

**Verdict: #90's premise SURVIVES as a real phase-2 problem. Root is a legitimate and architecturally cleaner answer to it than the virtual-HID option #83 proposed — but it is gated behind a host-OS change GMM cannot make for its users. #90 should record root-on-native-Linux as a known fifth option, explicitly parked on host-OS grounds rather than on merit.**

Concretely for #90's own option list: this research **strengthens** the "dedicated machine" option (a native-Linux box running Cuttlefish or redroid is a far better unattended substrate than a second Windows box) and **weakens** the "virtual HID driver" option (it solves less than #83 credited it with).

---

## Final recommendation

**v1: the PC client — Windows Graphics Capture plus `SendInput`. Unchanged from #83.**

**Phase 2 (unattended): keep the flow model substrate-agnostic. Do not commit to an input mechanism now.** If the unattended phase ever becomes a priority, the decision that actually matters is not "emulator or not" — it is **whether the "same Windows PC" constraint gets reopened.**

- **If the constraint holds:** the rootable-Android branch is closed. Not on preference, not on detection fear, but because the stock WSL2 kernel cannot boot these substrates and no primary source anywhere demonstrates GPU-accelerated Android inside WSL2. Revisit only if Microsoft ships binder/KVM in the default WSL2 kernel *and* someone publishes a working GPU-accelerated Android-in-WSL2 result. Both are checkable preconditions; neither is met today.
- **If the constraint is deliberately reopened in favour of a native-Linux host:** the branch becomes genuinely attractive, and the ranking is clear. **Cuttlefish first** — first-class `adb root` in the documented workflow, gfxstream GPU forwarding, Apache 2.0, the most active maintenance of the set, and Google's own canonical AOSP device. **redroid second** — clean containerization and simple root, but ~3 months stale, no LICENSE file, and unresolved binderfs issues. **Waydroid third** — healthy project and the best desktop-integration story, but its own docs recommend software rendering under VMs, and it is the one substrate with concrete HSR crash reports on record.

**Explicitly labelled as later-phase, not v1:** focus-independent input, minimized/headless operation, snapshots, and memory access are *all* phase-2 benefits. None of them improve attended v1, where the user is present and the window is foreground anyway. Any argument that credits v1 with these is mispriced.

**What I'd tell the dev in one line:** the pushback identified a real gap in #83 and a real capability that root uniquely provides — and then the evidence closed the door for an entirely different reason than #83 anticipated, one that is much harder to argue with than #83's original soft-factor accumulation. The right substrate for root is a Linux box, not a Windows box wearing WSL2.

---

## Confidence and open questions

**High confidence (primary sources, directly retrieved):**
- Stock WSL2 kernel lacks `CONFIG_KVM`, binder, binderfs, and ashmem ([config-wsl](https://github.com/microsoft/WSL2-Linux-Kernel/blob/master/Microsoft/config-wsl)).
- uinput/evdev has no focus concept; Android's `InputDispatcher` requires visible windows for touch and focused windows for keys (kernel.org docs; AOSP source).
- Cuttlefish is Linux/KVM-only with documented `adb root`, Apache 2.0, actively maintained (source.android.com; google/android-cuttlefish).
- Waydroid is GPLv3, Linux-only, actively maintained, and recommends software rendering under VMs (waydro.id; repo).
- Anbox is archived and unmaintained; Genymotion has no OSS edition; Android-x86 is stalled since 2021–22.
- Unity 6000.4 no longer offers generic x86 as an Android target architecture (Unity Manual).
- Google scopes its ARM translation to "application development and debug purposes" (Android Developers Blog).
- The HoYoPlay ToS Cheat Detection clause text, read first-hand — an improvement over #83, which could only paraphrase it.

**Medium confidence:**
- Real-world WSL2 vGPU overhead for a *headless* Android container. Microsoft's ~35% figure is for the WSLg compositor path specifically; the headless case is undocumented in either direction. I have not assumed the compositor penalty transfers.
- redroid's licensing. Apache-2.0 is asserted in README prose only; no LICENSE artifact exists.
- Waydroid app-level `su` (as opposed to container root). Container root is confirmed from source; app-visible root depends on third-party tooling.

**Low confidence / explicitly unresolved:**
- **Whether HSR would actually run acceptably on any rootable substrate.** The Waydroid crash reports are real but undiagnosed, and contradicted by reports of it working with tuning. Genuinely unknown.
- **Whether HSR integrates Play Integrity.** Unconfirmed. #83 inferred it; this research could not confirm it either. Still an inference.
- **libhoudini's characteristics.** No official documentation exists at all. Flagged rather than sourced.
- **Real playability of ARM64 Unity titles under x86 translation.** Only vendor marketing exists — one unverified "120 FPS" claim, and MuMu recommending specs well above HSR's own PC minimums. No independent benchmark located.
- **Android-x86 root-by-default.** No primary source found; community claims not cited.

**Fetch limitations encountered (same class as #83's):** HSR's Fair Gaming Declaration and PC Launcher FAQ are JS-rendered SPAs yielding only loading shells; the HoYoverse platform-support article returned HTTP 403; HoYoLAB threads did not render; Reddit was not reachable by the research tooling. A JS-capable browser pass would be needed to close the HoYoverse-policy questions properly — worth doing before shipping *any* automation feature, but it does not change this document's conclusion, which turns on the WSL2 kernel and GPU evidence.

**Checkable preconditions that would reopen this decision:**
1. Microsoft ships binder/binderfs/ashmem or KVM in the default WSL2 kernel.
2. A primary source demonstrates GPU-accelerated Android (Waydroid/redroid/Cuttlefish) inside WSL2 with real frame-rate data.
3. The "same Windows PC" constraint is deliberately reopened — at which point Cuttlefish on native Linux becomes the leading unattended substrate immediately, with no further research needed to justify a trial.
