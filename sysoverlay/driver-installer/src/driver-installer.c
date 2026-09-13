/**
 * driver-installer.c
 *
 * WinRing0 driver installer for AAXSysPulse. The driver (WinRing0x64.sys) is
 * embedded inside this exe as a resource, so the program ships as a single file.
 *
 * Goal: after running it, the driver is usable. Concretely:
 *   - WinRing0x64.sys is at C:\Windows\AAXSysPulse\WinRing0x64.sys
 *   - the AAXSysPulseWinRing0 kernel-driver service is registered
 *   - the driver is loaded (service RUNNING), OR the WinRing0 device
 *     \\.\WinRing0_1_2_0 is already provided by another running instance
 *
 * This installer is defensive: every realistic failure is detected, most are
 * self-healed, and each outcome gets a precise, actionable message.
 *
 * Steps:
 *   0. Ensure elevated (manifest requires admin; runtime check as fallback).
 *   1. Confirm dialog.
 *   2. Extract the embedded driver to a temp file.
 *   3. Ensure C:\Windows\AAXSysPulse\ exists (and is a directory).
 *   4. Copy the driver there (tolerating "file locked because loaded").
 *   5. Create-or-open the service, self-healing its binary path / start type.
 *   6. Start it, then verify RUNNING and/or probe the device.
 *
 * NOTE: The target path and service name MUST match the main program's loader
 * (see crates/win/src/msr.rs). The service is created with SERVICE_KERNEL_DRIVER
 * / SERVICE_DEMAND_START, identical to msr.rs, so the two are idempotent.
 */

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winsvc.h>
#include <shellapi.h>
#include <stdio.h>
#include <wchar.h>

/* Resource id/type the driver is embedded under (must match driver-installer.rc) */
#define SYS_RES_ID 101

static const wchar_t DRIVER_FILE[] = L"WinRing0x64.sys";
static const wchar_t TARGET_DIR[]  = L"C:\\Windows\\AAXSysPulse";
static const wchar_t SVC_NAME[]    = L"AAXSysPulseWinRing0";
static const wchar_t SVC_DISPLAY[] = L"AAXSysPulse WinRing0 MSR Driver";
static const wchar_t DEVICE_PATH[] = L"\\\\.\\WinRing0_1_2_0";

/* ---- outcome of the service step ---- */
enum {
    RES_ERROR = -1,       /* something failed; *err holds GetLastError()          */
    RES_OK_RUNNING,       /* our service is RUNNING                               */
    RES_OK_DEVICE,        /* device available (our service or another instance)   */
    RES_WARN_NOT_RUNNING  /* registered, but driver not loaded & device absent    */
};

typedef struct { int code; DWORD err; } Result;

/* ------------------------------------------------------------------ */
/* Small helpers                                                       */
/* ------------------------------------------------------------------ */

static BOOL is_elevated(void)
{
    BOOL admin = FALSE;
    SID_IDENTIFIER_AUTHORITY nt = SECURITY_NT_AUTHORITY;
    PSID grp = NULL;
    if (AllocateAndInitializeSid(&nt, 2, SECURITY_BUILTIN_DOMAIN_RID,
                                 DOMAIN_ALIAS_RID_ADMINS, 0, 0, 0, 0, 0, 0, &grp)) {
        CheckTokenMembership(NULL, grp, &admin);
        FreeSid(grp);
    }
    return admin;
}

/* Re-launch ourselves elevated (UAC). Returns TRUE if the elevated copy started. */
static BOOL relaunch_elevated(void)
{
    wchar_t exe[MAX_PATH];
    SHELLEXECUTEINFOW sei;
    if (!GetModuleFileNameW(NULL, exe, MAX_PATH)) return FALSE;
    ZeroMemory(&sei, sizeof(sei));
    sei.cbSize = sizeof(sei);
    sei.fMask = SEE_MASK_NOCLOSEPROCESS;
    sei.lpVerb = L"runas";
    sei.lpFile = exe;
    sei.nShow = SW_SHOWNORMAL;
    if (!ShellExecuteExW(&sei)) return FALSE;
    if (sei.hProcess) CloseHandle(sei.hProcess);
    return TRUE;
}

/* Is the WinRing0 device currently openable? (i.e. is a driver already loaded) */
static BOOL device_available(void)
{
    HANDLE h = CreateFileW(DEVICE_PATH, 0, FILE_SHARE_READ | FILE_SHARE_WRITE,
                           NULL, OPEN_EXISTING, 0, NULL);
    if (h == INVALID_HANDLE_VALUE) return FALSE;
    CloseHandle(h);
    return TRUE;
}

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

/* Read a service's current binary path and start type.
 * QueryServiceConfigW needs a right-sized buffer; returns FALSE on any error. */
static BOOL query_service_config(SC_HANDLE svc, wchar_t *bin, DWORD bin_cch,
                                 DWORD *start_type)
{
    DWORD needed = 0;
    QUERY_SERVICE_CONFIGW *q;

    QueryServiceConfigW(svc, NULL, 0, &needed);
    if (needed == 0) return FALSE;

    q = (QUERY_SERVICE_CONFIGW *)LocalAlloc(LMEM_FIXED, needed);
    if (!q) return FALSE;

    if (!QueryServiceConfigW(svc, q, needed, &needed)) {
        LocalFree((HLOCAL)q);
        return FALSE;
    }
    if (bin && bin_cch) {
        bin[0] = 0;
        if (q->lpBinaryPathName) lstrcpynW(bin, q->lpBinaryPathName, (int)bin_cch);
    }
    if (start_type) *start_type = q->dwStartType;
    LocalFree((HLOCAL)q);
    return TRUE;
}

/* ------------------------------------------------------------------ */
/* Service step: create-or-open + self-heal + start + verify           */
/* ------------------------------------------------------------------ */
static void install_service(const wchar_t *sys_path, Result *res)
{
    SC_HANDLE scm, svc;
    wchar_t cur_bin[1024];
    DWORD cur_start = 0;
    SERVICE_STATUS ss;
    BOOL running = FALSE;

    res->code = RES_ERROR;
    res->err = 0;

    /* 1. Open SCM */
    scm = OpenSCManagerW(NULL, NULL, SC_MANAGER_ALL_ACCESS);
    if (!scm) { res->err = GetLastError(); return; }

    /* 2. Create the service; if it exists, open it. */
    svc = CreateServiceW(scm, SVC_NAME, SVC_DISPLAY, SERVICE_ALL_ACCESS,
                         SERVICE_KERNEL_DRIVER, SERVICE_DEMAND_START,
                         SERVICE_ERROR_NORMAL, sys_path,
                         NULL, NULL, NULL, NULL, NULL);
    if (!svc) {
        DWORD e = GetLastError();
        if (e == ERROR_SERVICE_MARKED_FOR_DELETE) {
            /* A previous DeleteService is still pending: poll, then fall back
             * to opening it (a marked service can still be opened/queried). */
            int i;
            for (i = 0; i < 20 && !svc; i++) {
                Sleep(100);
                svc = CreateServiceW(scm, SVC_NAME, SVC_DISPLAY, SERVICE_ALL_ACCESS,
                                     SERVICE_KERNEL_DRIVER, SERVICE_DEMAND_START,
                                     SERVICE_ERROR_NORMAL, sys_path,
                                     NULL, NULL, NULL, NULL, NULL);
                if (!svc && GetLastError() != ERROR_SERVICE_MARKED_FOR_DELETE) break;
            }
            if (!svc) svc = OpenServiceW(scm, SVC_NAME, SERVICE_ALL_ACCESS);
        } else if (e == ERROR_SERVICE_EXISTS) {
            svc = OpenServiceW(scm, SVC_NAME, SERVICE_ALL_ACCESS);
        }
        if (!svc) { res->err = GetLastError(); CloseServiceHandle(scm); return; }
    }

    /* 3. Self-heal: repoint a stale binary path, re-enable a disabled service. */
    if (query_service_config(svc, cur_bin, ARRAYSIZE(cur_bin), &cur_start)) {
        BOOL fix_bin   = (_wcsicmp(cur_bin, sys_path) != 0);
        BOOL fix_start = (cur_start == SERVICE_DISABLED);
        if (fix_bin || fix_start) {
            if (!ChangeServiceConfigW(svc, SERVICE_NO_CHANGE,
                                      fix_start ? SERVICE_DEMAND_START : SERVICE_NO_CHANGE,
                                      SERVICE_NO_CHANGE,
                                      fix_bin ? sys_path : NULL,
                                      NULL, NULL, NULL, NULL, NULL, NULL)) {
                res->err = GetLastError();
                CloseServiceHandle(svc); CloseServiceHandle(scm);
                return;
            }
        }
    }

    /* 4. Start the driver. */
    if (!StartServiceW(svc, 0, NULL)) {
        DWORD e = GetLastError();
        if (e == ERROR_SERVICE_ALREADY_RUNNING) {
            /* fine */
        } else if (e == ERROR_SERVICE_DISABLED) {
            ChangeServiceConfigW(svc, SERVICE_NO_CHANGE, SERVICE_DEMAND_START,
                                 SERVICE_NO_CHANGE, NULL, NULL, NULL, NULL, NULL, NULL, NULL);
            if (!StartServiceW(svc, 0, NULL)) res->err = GetLastError();
        } else {
            res->err = e;   /* 183 = name collision, 1053 = timeout, signature, ... */
        }
    }

    /* 5. Verify actual state. */
    ZeroMemory(&ss, sizeof(ss));
    if (QueryServiceStatus(svc, &ss)) {
        if (ss.dwCurrentState == SERVICE_RUNNING) {
            running = TRUE;
        } else if (ss.dwWin32ExitCode != ERROR_SUCCESS) {
            res->err = ss.dwWin32ExitCode;   /* keep the driver's own failure code */
        }
    }

    CloseServiceHandle(svc);
    CloseServiceHandle(scm);

    /* 6. Functional check: is the device available? */
    if (running) {
        res->code = RES_OK_RUNNING;
    } else if (device_available()) {
        res->code = RES_OK_DEVICE;   /* another instance already provides it */
    } else {
        res->code = RES_WARN_NOT_RUNNING;
    }
}

/* ------------------------------------------------------------------ */
/* Message boxes                                                       */
/* ------------------------------------------------------------------ */
static void msg(UINT icon, const wchar_t *text)
{
    MessageBoxW(NULL, text, L"驱动安装", icon | MB_OK);
}

int WINAPI wWinMain(HINSTANCE h, HINSTANCE p, LPWSTR cmd, int show)
{
    wchar_t tmp[MAX_PATH];
    wchar_t dst[MAX_PATH];
    Result res;
    DWORD e;

    (void)h; (void)p; (void)cmd; (void)show;

    /* 0. Elevation (the embedded manifest requests admin; this is a fallback). */
    if (!is_elevated()) {
        if (relaunch_elevated()) return 0;   /* elevated copy takes over */
        msg(MB_ICONERROR,
            L"需要管理员权限。\n\n"
            L"请右键点击 driver-installer.exe，选择「以管理员身份运行」。");
        return 1;
    }

    /* 1. Confirm. */
    if (MessageBoxW(NULL,
            L"是否安装 AAXSysPulse 的 WinRing0 驱动？\n\n"
            L"本程序会将内嵌的 WinRing0x64.sys 解压并复制到：\n"
            L"C:\\Windows\\AAXSysPulse\\\n"
            L"并注册 AAXSysPulseWinRing0 驱动服务。",
            L"驱动安装",
            MB_ICONQUESTION | MB_YESNO | MB_DEFBUTTON1) != IDYES) {
        return 0;
    }

    /* 2. Extract the embedded driver. */
    if (!extract_driver_to_temp(tmp)) {
        msg(MB_ICONERROR,
            L"无法解压内嵌的驱动程序。\n\n"
            L"此可执行文件中缺少驱动资源，或临时文件夹不可用。\n"
            L"请重新下载 driver-installer.exe。");
        return 1;
    }

    wcscpy(dst, TARGET_DIR);
    wcscat(dst, L"\\");
    wcscat(dst, DRIVER_FILE);

    /* 3. Ensure the target folder exists and is a directory. */
    if (!CreateDirectoryW(TARGET_DIR, NULL)) {
        e = GetLastError();
        if (e == ERROR_ALREADY_EXISTS) {
            DWORD attr = GetFileAttributesW(TARGET_DIR);
            if (attr != INVALID_FILE_ATTRIBUTES && !(attr & FILE_ATTRIBUTE_DIRECTORY)) {
                msg(MB_ICONERROR,
                    L"C:\\Windows\\AAXSysPulse 已存在，但它是一个文件而非文件夹。\n\n"
                    L"请删除该文件后重新运行安装程序。");
                DeleteFileW(tmp);
                return 1;
            }
            /* else: it's already a directory -> fine */
        } else {
            wchar_t m[320];
            wsprintfW(m,
                L"无法创建文件夹 %s（错误 %lu）。\n\n"
                L"请检查系统盘的权限或剩余空间。",
                TARGET_DIR, e);
            msg(MB_ICONERROR, m);
            DeleteFileW(tmp);
            return 1;
        }
    }

    /* 4. Copy the driver file. */
    if (!CopyFileW(tmp, dst, FALSE)) {
        e = GetLastError();
        /* If the driver is already loaded, its .sys is locked and cannot be
         * overwritten. That is acceptable when the device is already up. */
        if ((e == ERROR_SHARING_VIOLATION || e == ERROR_ACCESS_DENIED) &&
            device_available()) {
            /* fall through to the service step */
        } else {
            wchar_t m[512];
            const wchar_t *hint =
                (e == ERROR_ACCESS_DENIED)
                    ? L"拒绝访问——请以管理员身份运行本安装程序。"
                : (e == ERROR_DISK_FULL)
                    ? L"系统盘空间已满。"
                : (e == ERROR_SHARING_VIOLATION)
                    ? L"文件正在使用（驱动已加载）。请重启后重试。"
                :   L"未知错误。";
            wsprintfW(m, L"无法将驱动复制到 %s（错误 %lu）。\n\n%s",
                      dst, e, hint);
            msg(MB_ICONERROR, m);
            DeleteFileW(tmp);
            return 1;
        }
    }
    DeleteFileW(tmp);

    /* 5. Register + start + verify. */
    install_service(dst, &res);

    switch (res.code) {
    case RES_OK_RUNNING:
        msg(MB_ICONINFORMATION,
            L"驱动安装成功。\n\n"
            L"WinRing0x64.sys 已复制到 C:\\Windows\\AAXSysPulse\\\n"
            L"且 AAXSysPulseWinRing0 服务正在运行。");
        return 0;

    case RES_OK_DEVICE:
        msg(MB_ICONINFORMATION,
            L"驱动服务已注册。\n\n"
            L"WinRing0x64.sys 已复制到 C:\\Windows\\AAXSysPulse\\\n"
            L"WinRing0 设备（\\.\\WinRing0_1_2_0）已由另一个正在运行的程序提供，\n"
            L"例如 Open Hardware Monitor / LibreHardware Monitor。\n"
            L"AAXSysPulse 主程序可直接使用它。");
        return 0;

    case RES_WARN_NOT_RUNNING: {
        wchar_t m[720];
        const wchar_t *hint =
            (res.err == ERROR_ALREADY_EXISTS)
                ? L"另一个程序已加载了 WinRing0 驱动。\n"
                  L"请关闭该程序（或重启）后重新运行本安装程序。"
            : (res.err == ERROR_SERVICE_MARKED_FOR_DELETE)
                ? L"上一次卸载尚未完成。请重启后重试。"
            : (res.err == ERROR_SERVICE_REQUEST_TIMEOUT)
                ? L"驱动加载超时。请重启后重试。"
            : (res.err == ERROR_ACCESS_DENIED)
                ? L"拒绝访问——请以管理员身份运行。"
            :   L"通常是驱动签名 / DSE（驱动强制签名）问题：请开启测试签名，\n"
                L"或在信任 WinRing0 证书的系统上运行。";
        wsprintfW(m,
            L"WinRing0x64.sys 已复制，AAXSysPulseWinRing0 服务也已注册，\n"
            L"但驱动未运行（错误 %lu）。\n\n%s\n\n"
            L"服务已安装；主程序将尝试再次启动它。",
            res.err, hint);
        msg(MB_ICONWARNING, m);
        return 0;
    }

    default: {   /* RES_ERROR: failed before/at registration */
        wchar_t m[640];
        const wchar_t *hint =
            (res.err == ERROR_ACCESS_DENIED)
                ? L"拒绝访问——请以管理员身份运行本安装程序。"
            : (res.err == ERROR_SERVICE_MARKED_FOR_DELETE)
                ? L"上一次卸载尚未完成。请重启后重试。"
            :   L"请参照上方错误代码；重启通常能清除 SCM 的临时状态。";
        wsprintfW(m,
            L"WinRing0x64.sys 已复制，但注册驱动服务失败\n"
            L"（错误 %lu）。\n\n%s",
            res.err, hint);
        msg(MB_ICONERROR, m);
        return 1;
    }
    }
}
