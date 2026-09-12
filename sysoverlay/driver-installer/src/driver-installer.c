/**
 * driver-installer.c
 *
 * Minimal WinRing0 driver installer. The driver (WinRing0x64.sys) is embedded
 * inside this exe as a resource, so the program ships as a single file.
 *
 * Flow:
 *   1. Show a native Windows dialog: "install the driver?"
 *   2. If yes, extract the embedded driver to a temp file
 *   3. Copy it into C:\Windows\SysOverlay\
 *   4. Report success or failure
 */

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
#include <wchar.h>

/* Resource id/type the driver is embedded under (must match driver-installer.rc) */
#define SYS_RES_ID 101

static const wchar_t DRIVER_FILE[] = L"WinRing0x64.sys";
static const wchar_t TARGET_DIR[]  = L"C:\\Windows\\SysOverlay";

/* Extract the embedded driver resource to a unique temp file.
 * On success returns TRUE and fills tmp_path (MAX_PATH); the caller must
 * DeleteFileW(tmp_path) when done. */
static BOOL extract_driver_to_temp(wchar_t *tmp_path)
{
    HRSRC res = FindResourceW(NULL, MAKEINTRESOURCEW(SYS_RES_ID), L"SYS");
    if (!res) return FALSE;
    HGLOBAL loaded = LoadResource(NULL, res);
    if (!loaded) return FALSE;
    DWORD size = SizeofResource(NULL, res);
    const void *data = LockResource(loaded);
    if (!data || size == 0) return FALSE;

    if (!GetTempPathW(MAX_PATH, tmp_path)) return FALSE;
    if (!GetTempFileNameW(tmp_path, L"sys", 0, tmp_path)) return FALSE;

    HANDLE f = CreateFileW(tmp_path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS,
                           FILE_ATTRIBUTE_NORMAL, NULL);
    if (f == INVALID_HANDLE_VALUE) return FALSE;

    DWORD written = 0;
    BOOL ok = WriteFile(f, data, size, &written, NULL);
    CloseHandle(f);
    if (!ok || written != size) {
        DeleteFileW(tmp_path);
        return FALSE;
    }
    return TRUE;
}

int WINAPI wWinMain(HINSTANCE h, HINSTANCE p, LPWSTR cmd, int show)
{
    (void)h; (void)p; (void)cmd; (void)show;

    int r = MessageBoxW(NULL,
        L"Install the SysOverlay WinRing0 driver?\n\n"
        L"This extracts WinRing0x64.sys from this program and copies it into:\n"
        L"C:\\Windows\\SysOverlay\\",
        L"Driver Install",
        MB_ICONQUESTION | MB_YESNO | MB_DEFBUTTON1);

    if (r != IDYES) {
        return 0;   /* user chose not to install */
    }

    /* 1. Extract the embedded driver to a temp file */
    wchar_t tmp[MAX_PATH];
    if (!extract_driver_to_temp(tmp)) {
        MessageBoxW(NULL,
            L"Failed to extract the embedded driver.\n\n"
            L"The driver resource is missing from this executable.",
            L"Driver Install", MB_ICONERROR | MB_OK);
        return 1;
    }

    /* 2. Destination: C:\Windows\SysOverlay\WinRing0x64.sys */
    wchar_t dst[MAX_PATH];
    wcscpy(dst, TARGET_DIR);
    wcscat(dst, L"\\");
    wcscat(dst, DRIVER_FILE);

    /* 3. Create the target folder (ignore "already exists") */
    if (!CreateDirectoryW(TARGET_DIR, NULL)) {
        DWORD e = GetLastError();
        if (e != ERROR_ALREADY_EXISTS) {
            wchar_t m[256];
            wsprintfW(m, L"Cannot create folder %s (error %lu).", TARGET_DIR, e);
            MessageBoxW(NULL, m, L"Driver Install", MB_ICONERROR | MB_OK);
            DeleteFileW(tmp);
            return 1;
        }
    }

    /* 4. Copy + report */
    BOOL copied = CopyFileW(tmp, dst, FALSE);
    if (copied) {
        MessageBoxW(NULL,
            L"Driver installed successfully.\n\n"
            L"WinRing0x64.sys is now in C:\\Windows\\SysOverlay\\",
            L"Driver Install", MB_ICONINFORMATION | MB_OK);
    } else {
        DWORD err = GetLastError();
        wchar_t msg[512];
        wsprintfW(msg,
            L"Failed to copy the driver (error %lu).\n\n"
            L"If this says \"Access is denied\", run driver-installer.exe as Administrator.",
            err);
        MessageBoxW(NULL, msg, L"Driver Install", MB_ICONERROR | MB_OK);
    }

    DeleteFileW(tmp);
    return copied ? 0 : 1;
}
