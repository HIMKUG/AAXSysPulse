# Driver Installer

A tiny Windows GUI helper that installs the WinRing0x64.sys driver for SysOverlay.
The driver is **embedded inside the executable**, so the program ships as a single
`.exe` with no external files.

## What it does

1. Show a native Windows dialog: **"install the driver?"**
2. If the user says yes, **extract the embedded driver** to a temp file
3. **Copy** it into `C:\Windows\SysOverlay\`
4. Report success or failure

No admin pre-checks, no service probing, no launching the main app — just
confirm → self-extract → copy → report.

> Note: writing into `C:\Windows\SysOverlay` requires Administrator rights. Run
> `driver-installer.exe` as Administrator (right-click → Run as administrator), or
> the copy will fail with "Access is denied" — the dialog tells you so.

## Building

### Prerequisites

- Windows 10/11
- Visual Studio 2022 **or** 2019 (Build Tools **or** Community edition)
- A Windows 10/11 SDK (any recent version — `build.bat` auto-detects the newest installed SDK)
- The driver binary **`WinRing0x64.sys`** placed next to `build.bat` (or `CMakeLists.txt`)

> The driver is embedded **at build time**. Without `WinRing0x64.sys` present, the
> build cannot bake it in (and the program would report "embedded driver missing"
> at runtime). Get it from OpenHardwareMonitor / LibreHardwareMonitor forks, or
> compile from OpenLibSys/WinRing0 source.

### Build with build.bat (recommended)

1. Put `WinRing0x64.sys` in this folder (same place as `build.bat`)
2. Run `build.bat` (double-click, or from a Developer Command Prompt)

```
cd D:\code\sys-overlay-c-master\sysoverlay\driver-installer
build.bat
```

`build.bat` auto-detects VS and the newest Windows SDK, and links with `/MT`
(static CRT) so the exe runs on machines without `vcruntime140.dll`.

### Build with CMake (alternative)

```
mkdir build && cd build
cmake ..
cmake --build . --config Release
```

CMake also embeds the driver, but requires `WinRing0x64.sys` next to
`CMakeLists.txt` at configure time.

## Usage

1. Build the driver-installer (the `WinRing0x64.sys` gets baked in)
2. Distribute / run the single `driver-installer.exe`
3. Right-click it → **Run as administrator**
4. Click **Yes** when asked to install; a dialog shows success or failure

No `WinRing0x64.sys` file is needed alongside the exe at runtime — it is already
inside it.

## Dialogs

- **Confirm** — "Install the SysOverlay WinRing0 driver?" (Yes / No)
- **Success** — "Driver installed successfully."
- **Failure** — Explains why (embedded driver missing, folder creation failed, or need admin rights)

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | User declined, or driver installed successfully |
| 1 | Could not install (embedded resource missing, folder creation failed, or copy failed) |

## Files

- `src/driver-installer.c` - Source code
- `driver-installer.rc` - Resource script (used by build.bat; embeds the driver)
- `driver-installer.rc.in` - CMake template for the resource script
- `build.bat` - Build script (recommended)
- `CMakeLists.txt` - CMake build configuration (alternative)
- `bin/driver-installer.exe` - Output executable
