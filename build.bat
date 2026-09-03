@echo off
setlocal
cd /d "%~dp0"

rem ASCII-only: cmd reads .bat in the OEM codepage, UTF-8 text breaks parsing.
set ZIG=E:\dev\zig\zig.exe

if not exist "%ZIG%" (
    echo Zig not found at %ZIG%
    echo Download from https://ziglang.org/download/ and unzip to E:\dev\zig
    exit /b 1
)

rem -Wl,--subsystem,windows is required: plain -mwindows is silently ignored by
rem zig cc, which would leave the exe as a console app (black window on launch).
rem -static keeps it free of libgcc/winpthread DLLs so the exe runs standalone.
"%ZIG%" cc -Os -g0 -std=c11 -Wall -Wextra -Wno-unused-parameter ^
    -ffunction-sections -fdata-sections -static ^
    -DUNICODE -D_UNICODE ^
    overlay.c -o sysoverlay-c.exe ^
    -Wl,--subsystem,windows -Wl,--gc-sections -Wl,-s ^
    -lpdh -lgdi32 -luser32 -ladvapi32 -lkernel32 -lcomdlg32
if errorlevel 1 exit /b 1

for %%F in (sysoverlay-c.exe) do echo built: sysoverlay-c.exe  %%~zF bytes
exit /b 0
