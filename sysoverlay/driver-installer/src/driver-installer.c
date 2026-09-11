/**
 * driver-installer.c
 *
 * Driver installer pre-launcher: checks admin rights, driver status,
 * and copies the driver file to C:\Windows\SysOverlay if needed.
 *
 * Exit codes:
 *   0 - Driver loaded or installed successfully
 *   1 - Admin rights required
 *   2 - Driver not loaded and cannot be installed
 *   3 - User cancelled
 */

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Service name (must match msr.rs) */
static const wchar_t SERVICE_NAME[] = L"SysOverlayWinRing0";
/* Driver file target directory */
static const wchar_t TARGET_DIR[]  = L"C:\\Windows\\SysOverlay";
/* Driver filename */
static const wchar_t DRIVER_FILE[] = L"WinRing0x64.sys";

/* ============================================================================
 * Utility Functions
 * ============================================================================ */

/* Check if running with admin privileges */
static BOOL is_admin(void)
{
    BOOL ret = FALSE;
    HANDLE tok = NULL;
    if (OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &tok)) {
        TOKEN_ELEVATION te;
        DWORD len = 0;
        if (GetTokenInformation(tok, TokenElevation, &te, sizeof(te), &len)) {
            ret = te.TokenIsElevated;
        }
        CloseHandle(tok);
    }
    return ret;
}

/* Check if driver service exists in SCM */
static BOOL service_exists(void)
{
    SC_HANDLE scm = OpenSCManagerW(NULL, NULL, SC_MANAGER_ENUMERATE_SERVICE);
    if (!scm) return FALSE;

    SC_HANDLE svc = OpenServiceW(scm, SERVICE_NAME, SERVICE_QUERY_STATUS);
    if (svc) {
        CloseServiceHandle(svc);
        CloseServiceHandle(scm);
        return TRUE;
    }
    CloseServiceHandle(scm);
    return FALSE;
}

/* Copy driver file to target directory */
static BOOL install_driver_file(const wchar_t *src_path)
{
    /* Create target directory if it doesn't exist */
    if (!CreateDirectoryW(TARGET_DIR, NULL)) {
        DWORD err = GetLastError();
        if (err != ERROR_ALREADY_EXISTS) {
            wchar_t msg[512];
            wsprintfW(msg, L"Failed to create directory: %s (Error: %lu)", TARGET_DIR, err);
            MessageBoxW(NULL, msg, L"Driver Install", MB_ICONERROR | MB_OK);
            return FALSE;
        }
    }

    /* Build destination path */
    wchar_t dst_path[MAX_PATH];
    wcscpy(dst_path, TARGET_DIR);
    wcscat(dst_path, L"\\");
    wcscat(dst_path, DRIVER_FILE);

    /* Copy file */
    if (!CopyFileW(src_path, dst_path, FALSE)) {
        wchar_t msg[512];
        DWORD err = GetLastError();
        wsprintfW(msg, L"Failed to copy driver file.\nFrom: %s\nTo: %s\nError: %lu",
                  src_path, dst_path, err);
        MessageBoxW(NULL, msg, L"Driver Install", MB_ICONERROR | MB_OK);
        return FALSE;
    }

    return TRUE;
}

/* Try to install from current directory or embedded resource */
static BOOL install_from_resource(void)
{
    HRSRC res = FindResourceW(NULL, MAKEINTRESOURCEW(101), L"SYS");
    if (!res) {
        /* Try loading from current directory */
        wchar_t cur_dir[MAX_PATH];
        if (!GetModuleFileNameW(NULL, cur_dir, MAX_PATH)) return FALSE;

        wchar_t *slash = wcsrchr(cur_dir, L'\\');
        if (slash) slash[1] = L'\0';
        wcscat(cur_dir, DRIVER_FILE);

        return install_driver_file(cur_dir);
    }

    HGLOBAL loaded = LoadResource(NULL, res);
    if (!loaded) return FALSE;

    DWORD size = SizeofResource(NULL, res);
    const void *data = LockResource(loaded);

    /* Create temp file */
    wchar_t tmp_path[MAX_PATH];
    GetTempPathW(MAX_PATH, tmp_path);
    wcscat(tmp_path, DRIVER_FILE);

    HANDLE f = CreateFileW(tmp_path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS,
                            FILE_ATTRIBUTE_TEMPORARY, NULL);
    if (f == INVALID_HANDLE_VALUE) return FALSE;

    DWORD written = 0;
    WriteFile(f, data, size, &written, NULL);
    CloseHandle(f);

    BOOL ret = install_driver_file(tmp_path);

    /* Clean up temp file */
    DeleteFileW(tmp_path);

    return ret;
}

/* ============================================================================
 * Dialog Results
 * ============================================================================ */

typedef enum {
    DLG_RESULT_INSTALL,
    DLG_RESULT_LAUNCH,
    DLG_RESULT_ABORT
} DLG_RESULT;

/* Show admin warning dialog */
static DLG_RESULT show_admin_warning(void)
{
    int ret = MessageBoxW(
        NULL,
        L"[Admin Rights Required]\n\n"
        L"Driver installation requires administrator privileges.\n"
        L"Do you want to launch the app anyway? (Power monitoring will be unavailable)\n\n"
        L"Click OK to continue, or Cancel to exit.",
        L"Driver Install - Insufficient Privileges",
        MB_ICONWARNING | MB_OKCANCEL | MB_DEFBUTTON2
    );
    return (ret == IDOK) ? DLG_RESULT_LAUNCH : DLG_RESULT_ABORT;
}

/* Show driver missing warning dialog */
static DLG_RESULT show_driver_missing_warning(void)
{
    int ret = MessageBoxW(
        NULL,
        L"[Driver Not Loaded]\n\n"
        L"SysOverlay WinRing0 MSR driver is not installed.\n"
        L"Power monitoring will be unavailable.\n\n"
        L"Do you want to continue launching the app?",
        L"Driver Install",
        MB_ICONWARNING | MB_OKCANCEL | MB_DEFBUTTON2
    );
    return (ret == IDOK) ? DLG_RESULT_LAUNCH : DLG_RESULT_ABORT;
}

/* Show install confirmation dialog */
static DLG_RESULT show_install_prompt(void)
{
    wchar_t msg[512];
    wsprintfW(msg,
        L"[Driver Installation]\n\n"
        L"About to install SysOverlay WinRing0 MSR driver.\n"
        L"This will copy the driver to C:\\Windows\\SysOverlay.\n\n"
        L"Click OK to install and launch, or Cancel to skip.",
        0);
    int ret = MessageBoxW(NULL, msg, L"Driver Install",
        MB_ICONINFORMATION | MB_OKCANCEL | MB_DEFBUTTON1);
    return (ret == IDOK) ? DLG_RESULT_INSTALL : DLG_RESULT_LAUNCH;
}

/* Show install result dialog */
static void show_install_result(BOOL success)
{
    if (success) {
        MessageBoxW(NULL,
            L"Driver installed successfully!\n\n"
            L"Power monitoring is ready.",
            L"Driver Install",
            MB_ICONINFORMATION | MB_OK);
    } else {
        MessageBoxW(NULL,
            L"Driver installation failed.\n\n"
            L"Power monitoring will be unavailable.",
            L"Driver Install",
            MB_ICONERROR | MB_OK);
    }
}

/* ============================================================================
 * Main Flow
 * ============================================================================ */

/* Launch the main application */
static void launch_main_app(void)
{
    wchar_t exe_path[MAX_PATH];
    if (!GetModuleFileNameW(NULL, exe_path, MAX_PATH)) {
        MessageBoxW(NULL, L"Cannot determine main app path", L"Error", MB_ICONERROR | MB_OK);
        ExitProcess(3);
    }

    /* Launch with DETACHED_PROCESS so it detaches from this console */
    STARTUPINFOW si = { sizeof(si) };
    PROCESS_INFORMATION pi = { 0 };

    if (CreateProcessW(exe_path, NULL, NULL, NULL, FALSE,
                        DETACHED_PROCESS, NULL, NULL, &si, &pi)) {
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    } else {
        wchar_t msg[512];
        wsprintfW(msg, L"Failed to launch main application.\nPath: %s\nError: %lu",
                  exe_path, GetLastError());
        MessageBoxW(NULL, msg, L"Error", MB_ICONERROR | MB_OK);
    }
}

/* Check-only mode: exit with code based on driver status */
static void run_check_only(void)
{
    if (service_exists()) {
        ExitProcess(0);
    } else {
        ExitProcess(2);
    }
}

int WINAPI wWinMain(HINSTANCE h, HINSTANCE p, LPWSTR cmd, int show)
{
    (void)h; (void)p; (void)show;

    /* Parse command line */
    if (cmd && wcsstr(cmd, L"--check-only")) {
        run_check_only();
        return 0;
    }

    /* Check admin rights */
    BOOL admin = is_admin();
    BOOL loaded = service_exists();

    if (!admin) {
        /* No admin privileges */
        DLG_RESULT r = show_admin_warning();
        if (r == DLG_RESULT_ABORT) {
            ExitProcess(3);
        }
        /* User chose to launch anyway */
        launch_main_app();
        ExitProcess(0);
    }

    if (!loaded) {
        /* Driver not loaded, ask user */
        DLG_RESULT r = show_driver_missing_warning();
        if (r == DLG_RESULT_LAUNCH) {
            launch_main_app();
            ExitProcess(0);
        }

        /* User wants to install */
        r = show_install_prompt();
        if (r == DLG_RESULT_LAUNCH) {
            launch_main_app();
            ExitProcess(0);
        }

        /* Perform installation */
        BOOL ok = install_from_resource();
        show_install_result(ok);
    }

    /* Driver loaded or installed, launch main app */
    launch_main_app();
    return 0;
}
