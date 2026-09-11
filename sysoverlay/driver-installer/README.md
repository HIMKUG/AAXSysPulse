# Driver Installer

A Windows GUI pre-launcher for SysOverlay that handles WinRing0x64.sys driver installation.

## Purpose

Before running SysOverlay (the main application), run this `driver-installer.exe` first. It will:

1. **Check for administrator privileges** - Required for driver installation
2. **Check if the driver is already loaded** - Skip installation if present
3. **Prompt to install the driver** - Copy WinRing0x64.sys to `C:\Windows\SysOverlay\`
4. **Launch the main application** - Automatically starts SysOverlay

## Building

### Prerequisites

- Windows 10/11
- Visual Studio 2022 Build Tools with MSVC 14.44 or later
- Windows 10 SDK (10.0.26100.0 or compatible)

### Build Steps

1. Open a Developer Command Prompt for VS 2022
2. Navigate to this directory
3. Run `build.bat`

```
cd D:\code\sys-overlay-c-master\sysoverlay\driver-installer
build.bat
```

Or simply double-click `build.bat` from Explorer.

## Usage

1. Build the driver-installer using `build.bat`
2. Place `driver-installer.exe` **alongside** `SysOverlay.exe`
3. Double-click `driver-installer.exe` instead of `SysOverlay.exe`
4. Follow the on-screen dialogs

## Dialogs

- **Admin Warning** - Appears if not running as administrator. Choose to launch anyway (power monitoring disabled) or exit.
- **Driver Missing Warning** - Appears if driver not loaded. Choose to continue or install.
- **Install Confirmation** - Confirms driver installation to `C:\Windows\SysOverlay\`
- **Install Result** - Shows success or failure

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Driver loaded or installed successfully |
| 1 | Admin rights required (user chose to launch anyway) |
| 2 | Driver not available (user chose to launch anyway) |
| 3 | User cancelled |

## Driver Location

The driver file `WinRing0x64.sys` should be placed **alongside** `driver-installer.exe` in the same directory. The installer will copy it to:

```
C:\Windows\SysOverlay\WinRing0x64.sys
```

You can obtain WinRing0x64.sys from:
- OpenHardwareMonitor project
- NGAudio/LibreHardwareMonitor forks
- Or compile from OpenLibSys/WinRing0 source

## Architecture

```
User launches driver-installer.exe
    |
    v
[Admin Check] --> [No Admin] --> Show warning --> [User chooses to launch anyway]
    |                                                       |
    v                                                       v
[Driver Check] --> [Driver Missing] --> Show prompt --> [User chooses to install]
    |                                       |
    v                                       v
[Driver Loaded] --> Launch main app <------+
    |
    v
Launch main app (SysOverlay.exe)
```

## Files

- `src/driver-installer.c` - Source code
- `build.bat` - Build script
- `bin/driver-installer.exe` - Output executable
- `CMakeLists.txt` - CMake build configuration (alternative to build.bat)
