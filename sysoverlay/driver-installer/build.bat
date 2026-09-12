@echo off
REM Build script for driver-installer
REM Requires: Visual Studio 2022/2019 (Build Tools or Community) with a Windows 10/11 SDK
REM
REM What it does:
REM   1. Compiles driver-installer.rc with rc.exe -> embeds WinRing0x64.sys into the exe
REM   2. Compiles driver-installer.c and links the resource in
REM
REM Improvements:
REM   * Auto-detects vcvars64.bat across common VS layouts (BuildTools / Community, x86 / x64).
REM   * Auto-detects the newest installed Windows SDK (no hard-coded version).
REM   * Links with /MT (static CRT): the exe runs on machines without vcruntime140.dll.
REM   * /utf-8: source is always read as UTF-8.

setlocal EnableDelayedExpansion

set SOURCE=%~dp0src\driver-installer.c
set RC=%~dp0driver-installer.rc
set RES=%~dp0obj\driver-installer.res
set OUTPUT=%~dp0bin\driver-installer.exe
set SYS=%~dp0WinRing0x64.sys

REM Build from this directory so the .rc can find WinRing0x64.sys by relative path.
cd /d "%~dp0"

REM --- The driver binary must be present at build time (it gets embedded) ---
if not exist "%SYS%" (
    echo ERROR: WinRing0x64.sys not found at:
    echo        %SYS%
    echo.
    echo        Drop the WinRing0x64.sys driver next to this build.bat, then run again.
    exit /b 1
)

REM --- Locate vcvars64.bat (try several common VS layouts) ---
set VCVARS=
for %%P in (
  "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
  "C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
  "C:\Program Files (x86)\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
  "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
  "C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
  "C:\Program Files (x86)\Microsoft Visual Studio\2019\Community\VC\Auxiliary\Build\vcvars64.bat"
) do (
  if not defined VCVARS if exist %%P set "VCVARS=%%P"
)
if not defined VCVARS (
    echo ERROR: vcvars64.bat not found. Install Visual Studio 2022/2019 Build Tools.
    exit /b 1
)

REM --- Locate the newest Windows SDK under the Kits folder ---
set SDKROOT=C:\Program Files (x86)\Windows Kits\10
set SDKVER=
for /f "delims=" %%D in ('dir /b /ad /o-n "%SDKROOT%\Include" 2^>nul') do (
  if not defined SDKVER set "SDKVER=%%D"
)
if not defined SDKVER (
    echo ERROR: Windows SDK not found under %SDKROOT%\Include
    exit /b 1
)

REM --- rc.exe (resource compiler) ---
set RC_EXE=%SDKROOT%\bin\%SDKVER%\x64\rc.exe
if not exist "%RC_EXE%" (
    echo ERROR: rc.exe not found at %RC_EXE%
    exit /b 1
)

REM --- Output directories ---
if not exist "%~dp0obj" mkdir "%~dp0obj"
if not exist "%~dp0bin" mkdir "%~dp0bin"

REM --- Set up VS environment (defines INCLUDE/LIB for the toolset) ---
call "%VCVARS%" >nul 2>&1

set INCLUDE=%SDKROOT%\Include\%SDKVER%\ucrt;%SDKROOT%\Include\%SDKVER%\um;%SDKROOT%\Include\%SDKVER%\shared;%SDKROOT%\Include\%SDKVER%\winrt;%INCLUDE%
set LIB=%SDKROOT%\Lib\%SDKVER%\ucrt\x64;%SDKROOT%\Lib\%SDKVER%\um\x64;%LIB%

REM --- Step 1: embed the driver resource ---
"%RC_EXE%" /nologo /fo "%RES%" "%RC%"
if errorlevel 1 (
    echo Resource compilation failed.
    exit /b 1
)

REM --- Step 2: compile + link (static CRT, no console) ---
cl.exe /O1 /Os /GS- /DUNICODE /D_UNICODE /D_CRT_SECURE_NO_WARNINGS /EHsc /MT /W3 /utf-8 ^
    /Fe:"%OUTPUT%" "%SOURCE%" "%RES%" ^
    /link /SUBSYSTEM:WINDOWS /MACHINE:X64 kernel32.lib advapi32.lib user32.lib

if %ERRORLEVEL% EQU 0 (
    echo Build successful: %OUTPUT%
) else (
    echo Build failed with error code %ERRORLEVEL%
    exit /b %ERRORLEVEL%
)
