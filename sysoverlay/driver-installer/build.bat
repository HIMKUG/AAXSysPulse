@echo off
REM Build script for driver-installer
REM Requires: Visual Studio 2022 Build Tools with MSVC 14.44 or later

set VCVARS="C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
set SDK_INC=C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0
set SDK_LIB=C:\Program Files (x86)\Windows Kits\10\Lib\10.0.26100.0
set SOURCE=%~dp0src\driver-installer.c
set OUTPUT=%~dp0bin\driver-installer.exe

REM Create output directory
if not exist "%~dp0bin" mkdir "%~dp0bin"

REM Set up VS environment
call %VCVARS% >nul 2>&1

REM Add Windows SDK include paths
set INCLUDE=%SDK_INC%\ucrt;%SDK_INC%\um;%SDK_INC%\shared;%SDK_INC%\winrt;%INCLUDE%
set LIB=%SDK_LIB%\ucrt\x64;%SDK_LIB%\um\x64;%LIB%

REM Compile
cl.exe /O1 /Os /GS- /DUNICODE /D_UNICODE /EHsc /MD /W4 /Fe:"%OUTPUT%" "%SOURCE%" /link /SUBSYSTEM:WINDOWS /MACHINE:X64 kernel32.lib advapi32.lib user32.lib

if %ERRORLEVEL% EQU 0 (
    echo Build successful: %OUTPUT%
) else (
    echo Build failed with error code %ERRORLEVEL%
    exit /b %ERRORLEVEL%
)
