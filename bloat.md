# Windows scan & debloat — session log

Everything done in this chat about cleaning and debloating Windows,
kept as the evidence base for the OS Cleaner app (see
[README.md](./README.md)). Numbers were verified by before/after
measurement, not estimates.

**Machine:** Windows 11 Pro, build 26300, MSI laptop, RTX 5070 Ti Laptop
GPU, Samsung 980 Pro 2TB + WD 2TB. **C: free: 18.87GB → 30.17GB
(+11.3GB)**, with ~15.6GB more pending (reboot + bake period).

---

## 1. MSYS2 relocation (context: Cloud9 server work)

- Found `C:\msys64` (1.41GB, MSYS2 20260927, installed 2026-11-16 via
  WinGet) holding the SSH/rsync/gcc toolchain used for server ops.
- MSYS2 is not folder-movable (absolute paths baked into install
  records), so migrated properly: hash-verified same-version
  installer → `C:\dev\msys64`, full update, same package set
  (base, base-devel, gcc, mingw-w64-ucrt gcc, openssh, rsync,
  sshpass + m4/bison/flex/dos2unix parity), `home/.ssh` restored.
- Verified with tool versions + compile-and-run test (exit 42).
- Old install uninstalled via WinGet; registry/shortcuts/WinGet now
  point at the new path; Ruby's separate bundled msys64 untouched.
- Added `ucrt64\bin` + `usr\bin` to user PATH (appended, so no
  shadowing of system tools).

## 2. C:\Windows — the "60GB" investigation

### 2.1 Breakdown (robocopy-measured)

| Folder | Reported | Truth |
|---|---|---|
| WinSxS | 18.49GB | Hardlink-inflated; Backup 0.36 + Temp 0.17 only real cruft |
| System32 | 17.95GB | Shares bytes with WinSxS; incl. DriverStore 11.97GB |
| Installer | 6.47GB | 336 .msi (6.25GB) + 1 .msp; 50 orphans = 1.6GB |
| assembly (NGEN) | 4.33GB | Self-maintained perf cache — left alone |
| SystemApps/SysWOW64/.NET/etc | ~4GB | Normal OS payload |
| SoftwareDistribution/Temp/Panther/Logs | ~0 | Already clean |

Key insight: Explorer double-counts WinSxS↔System32 hardlinks, so the
"60GB" was partly an accounting illusion. Real waste found: ~6.5GB.

### 2.2 Installer orphans (method future app should reuse)

- Read every cached .msi ProductName via WindowsInstaller COM.
- Built the referenced set from ALL `LocalPackage` registry values
  (HKLM Installer\UserData recursive + HKCU Installer).
- 50 files unreferenced = 1598MB: NVIDIA Nsight duplicate installers
  (617+491MB), stale Edge (193MB), patch (67MB), 3× old Apple Mobile
  Device (114MB), old Office/Acrobat/XNA/SQL/AppVerifier copies.
- Microsoft offers no supported orphan cleaner (docs say leave the
  folder alone), so: move-to-backup-first, delete after 2 weeks.

### 2.3 Stale drivers

- DriverStore 11.97GB across 133 packages; top: 2× NVIDIA nvmisi
  2.9GB + 2× Intel iigd 2.2/2.0GB.
- Active driver verified (oem9.inf, RTX 32.0.16.2140); removed
  superseded oem62 (NVIDIA) + oem82 (Intel) via pnputil ≈ 4.9GB.
- Minor dupes (WiFi, audio) left as not worth the risk/round-trip.

### 2.4 Research + execution

- Used the `research` skill (background agent, primary Microsoft
  sources only) → findings in `%TEMP%\mswin60-research.md`.
- Verdicts applied: DISM StartComponentCleanup WITHOUT ResetBase
  (keeps updates uninstallable); never hand-delete WinSxS/Installer/
  NGEN/servicing; Disk Cleanup/Storage Sense marginal here.
- First elevated run hung 30 min in DISM AnalyzeComponentStore
  (0 CPU after tree walk — known-flaky path). Killed it, reran
  without Analyze in the order drivers → orphans → DISM cleanup.
  Rerun: exit 0 in ~8 min. Lesson for the app: never depend on
  Analyze; order big wins first; heartbeat-monitor DISM calls.

## 3. AppX debloat

- Researched Chris Titus Tech's WinUtil from its GitHub raw files:
  its blanket "remove all Store apps" list is legacy and explicitly
  NOT RECOMMENDED by its authors; current WinUtil does targeted
  removals (Widgets/Edge/OneDrive/Copilot) + per-app UI.
- Inventoried user AppX with sizes. Removed 7 (Clipchamp, Weather,
  News, Alarms, Sound Recorder, To Do, Dev Home), provisioned copies
  absent. Kept: PC Manager, Phone Link (+CrossDevice), Widgets,
  Media Player, Photos, all System/framework packages, all user and
  hardware apps. Tier 2 (Snipping Tool, Samsung trio, IP camera)
  deferred for explicit confirmation.

## 4. C:\ProgramData (27GB)

- `Microsoft\Windows\Containers` 15.9GB = one 14.3GB Windows Sandbox
  base image (+1.6GB store). User confirmed Sandbox unused →
  disabled the `Containers-DisposableClientVM` feature (supported
  removal). Frees on REBOOT — verification pending.
- Remnants removed (elevated): Dragon Center UWP 636MB (superseded
  by MSI Center), Samsung Magician data + setup backup 394MB,
  GeForce Experience logs 95MB (replaced by NVIDIA App), Avast
  Icarus 41MB. SamsungMagicianSVC deleted; SamsungAccountService
  kept (owned by installed SamsungAccount app).
- Deliberately kept: Package Cache 2.35GB (uninstall/repair data),
  VS installer data 2.34GB (CUDA/Nsight), NGX models 481MB
  (Broadcast/DLSS), CameraRaw 836MB (serves Photoshop CC 2019!),
  MetaQuotes, Defender, live app data.

## 5. C:\Program Files scan

- Mostly the owner's legit dev/streamer toolbox — kept: VS 2022
  Community 5.24GB (Nsight VSE depends on it), CUDA toolkit 4.71GB,
  Nsight suite, Docker, dotnet (single SDK 8.0.428, clean),
  Git/Go/Python/CMake, streamer apps, utilities.
- Verified benign (kept): Samsung USB drivers, Malwarebytes Windows
  Firewall Control, Everything (configured), Npcap (Wireshark dep).
- Micro-candidates: 3 empty remnant dirs (Avast, EA Games,
  UrBackupServer), HD Tune doc leftovers, Monect dll — ~1MB total.
- Flagged: KMSpico activator (25MB, piracy + malware risk; removal
  needs a legit-license decision by owner).
- Noted for later: `Program Files (x86)` is 27GB (unscanned);
  user already owns TreeSize Free, WinDirStat, and WizTree
  (validates the app's feature direction).

## 6. Pending items (for a later session)

1. Reboot → verify 14GB Sandbox store actually cleared.
2. ~2 weeks: delete `C:\Installer-Orphans-Backup-20261125` (+1.6GB).
3. Tier-2 AppX (Snipping/Samsung/IPCam) if owner confirms.
4. `Program Files (x86)` 27GB scan.
5. KMSpico decision (needs legit license path).
