/* AAXSysPulse —— 纯 Win32 悬浮系统监控卡片
 *
 * 只链 user32 / gdi32 / pdh / advapi32：不依赖 GDI+、不依赖 C++ 运行时。
 *
 * 透明做法：先用 GDI 画一张完全不透明的卡片，再把同一个圆角矩形按 2 倍尺寸
 * 画成蒙版、下采样得到边缘覆盖率，两者合成成预乘 alpha 交给 UpdateLayeredWindow。
 * 圆角因此是平滑的，整卡透明度均匀（等价于"窗口不透明度"）。
 */

#include <windows.h>
#include <pdh.h>
#include <pdhmsg.h>      /* PDH_MORE_DATA / PDH_CSTATUS_* 的权威定义在这个消息头里 */
#include <commdlg.h>     /* ChooseColorW 颜色对话框 */
#include <strsafe.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>

typedef struct { DWORD CStatus; double doubleValue; } FmtValue;
typedef struct { WCHAR *szName; FmtValue v; } FmtItem;
_Static_assert(sizeof(FmtValue) == 16, "FmtValue 布局必须与 PDH_FMT_COUNTERVALUE 一致");
_Static_assert(sizeof(FmtItem) == 24, "FmtItem 布局必须与 PDH_FMT_COUNTERVALUE_ITEM_W 一致");

#define CARD_W_L  268          /* 96dpi 下的逻辑尺寸 */
#define CARD_H_L  184
#define MIN_H_L   36           /* 极简模式的逻辑高度 */
#define MIN_W_L   320          /* 极简模式加宽：单行容纳四项指标 */
#define MINV_W_L  128          /* 极简单列：四项各占一行 */
#define MINV_H_L  96
#define MINV2_W_L 88           /* 极简分列：名称/数值各一行，下载、上传各一行 */
#define MINV2_H_L 162
#define PAD_X     16
#define RADIUS    14
#define MARGIN    24

#define IDM_TOPMOST 1
#define IDM_MINIMAL 2
#define IDM_HOVER   3
#define IDM_BGCLR   6
#define IDM_FGCLR   7
#define IDM_CLRDEF  8
#define IDM_QUIT    9
#define IDM_OP(i)   (10 + (i))
#define IDM_IV(i)   (20 + (i))
#define IDM_ZOOM(i) (30 + (i))
#define IDM_UNIT(i) (40 + (i))
#define IDM_SHAPE(i) (50 + (i))
#define IDM_VERT(i) (60 + (i))

static const int OPACITY_PCT[10] = {10, 20, 30, 40, 50, 60, 70, 80, 90, 100};
static const int INTERVAL_DDJ[3] = {5, 10, 20};   /* 秒 x10 */
static const int ZOOM_PCT[5]     = {75, 100, 125, 150, 200};

/* 用户可调：背景/主文字色（菜单选色，写入配置）；其余角色色从这两色派生 */
static COLORREF g_bgColor = RGB(20, 22, 27);
static COLORREF g_fgColor = RGB(238, 241, 244);
static COLORREF g_cBorder, g_cTrack, g_cTitle, g_cSub, g_cName;

#define CLR_BG     g_bgColor
#define CLR_BORDER g_cBorder
#define CLR_TRACK  g_cTrack
#define CLR_TITLE  g_cTitle
#define CLR_SUB    g_cSub
#define CLR_NAME   g_cName
#define CLR_VALUE  g_fgColor
#define CLR_OK     RGB(74, 222, 128)
#define CLR_WARN   RGB(251, 191, 36)
#define CLR_HOT    RGB(248, 113, 113)
#define CLR_IDLE   RGB(63, 69, 80)

typedef struct {
    double cpu, mem, gpu, memUsedGb, memTotalGb;
    double netR, netS;                 /* MB/s */
    double uptimeSec;
} Metrics;

static PDH_HQUERY   g_q = NULL;
static PDH_HCOUNTER g_gpu = NULL, g_nr = NULL, g_ns = NULL;
static BOOL         g_pdhOk = FALSE;

static HWND  g_hwnd = NULL;
static HFONT g_uiFont = NULL, g_monoFont = NULL;
static double g_scale = 1.0;
static int    g_cardW = CARD_W_L, g_cardH = CARD_H_L;

static int  g_opacityPct = 90, g_intervalDdj = 10;
static int  g_zoomPct = 100;
static BOOL g_minimal = FALSE;
static BOOL g_hoverMode = FALSE;      /* 悬停显示：平时近隐身，鼠标压上才点亮 */
static BOOL g_hovering = FALSE;       /* 鼠标当前是否在卡片上 */
static DWORD g_revealUntil = 0;       /* 启动后先点亮到这个时刻，避免重启后找不到卡片 */
static int  g_netUnit = 1;            /* 网速单位：0=KB/s 1=MB/s 2=GB/s */
static int  g_shape = 0;              /* 外形：0=圆角 1=直角 2=纯内容(无背景) */
static int  g_vert = 0;               /* 极简排列：0=单行 1=单列 2=分列 */
static BOOL g_modalHold = FALSE;      /* 右键菜单/颜色对话框打开期间：暂停置顶保持与悬停隐藏 */
static BOOL g_topmost = TRUE;
static int  g_posX = -99999, g_posY = -99999;
static BOOL g_dragging = FALSE, g_dragMoved = FALSE;
static POINT g_dragCursor0, g_dragWin0;

static Metrics g_m;
static BOOL    g_mValid = FALSE;
static ULARGE_INTEGER g_prevIdle, g_prevKernel, g_prevUser;
static BOOL g_haveTimes = FALSE;

static int px(int logical) { return (int)(logical * g_scale * g_zoomPct / 100.0 + 0.5); }

/* 尺寸由"模式 × DPI × 用户档位"共同决定；换档/换模式/初始化后调用。
 * 窗口实际大小在 repaint() 里经 UpdateLayeredWindow 的 sz 参数生效，无需 SetWindowPos */
static void refreshLayout(void)
{
    if (g_minimal && g_vert == 1) {
        g_cardW = px(MINV_W_L);
        g_cardH = px(MINV_H_L);
    } else if (g_minimal && g_vert == 2) {
        g_cardW = px(MINV2_W_L);
        g_cardH = px(MINV2_H_L);
    } else if (g_minimal) {
        g_cardW = px(MIN_W_L);
        g_cardH = px(MIN_H_L);
    } else {
        g_cardW = px(CARD_W_L);
        g_cardH = px(CARD_H_L);
    }
}

/* ---------------- 配置 ---------------- */

static FILE *openCfg(const char *mode)
{
    const char *ad = getenv("APPDATA");
    WCHAR dir[MAX_PATH * 2];
    WCHAR file[MAX_PATH * 2];
    char mb[MAX_PATH * 3];
    FILE *f;

    if (ad) MultiByteToWideChar(CP_UTF8, 0, ad, -1, dir, MAX_PATH);
    else    StringCchCopyW(dir, ARRAYSIZE(dir), L".");

    /* One-time migration: rename the old %APPDATA%\SysOverlayC folder. */
    if (ad) {
        WCHAR old[MAX_PATH * 2];
        WCHAR neo[MAX_PATH * 2];
        StringCchCopyW(old, ARRAYSIZE(old), dir);
        StringCchCatW(old, ARRAYSIZE(old), L"\\SysOverlayC");
        StringCchCopyW(neo, ARRAYSIZE(neo), dir);
        StringCchCatW(neo, ARRAYSIZE(neo), L"\\AAXSysPulse");
        if (GetFileAttributesW(old) != INVALID_FILE_ATTRIBUTES &&
            GetFileAttributesW(neo) == INVALID_FILE_ATTRIBUTES) {
            MoveFileExW(old, neo, MOVEFILE_COPY_ALLOWED | MOVEFILE_WRITE_THROUGH);
        }
    }

    StringCchCopyW(file, ARRAYSIZE(file), dir);
    StringCchCatW(file, ARRAYSIZE(file), L"\\AAXSysPulse");
    CreateDirectoryW(file, NULL);
    StringCchCatW(file, ARRAYSIZE(file), L"\\config.txt");
    WideCharToMultiByte(CP_UTF8, 0, file, -1, mb, sizeof(mb), NULL, NULL);
    f = fopen(mb, mode);
    return f;
}

static void loadConfig(void)
{
    FILE *f = openCfg("r");
    char line[128];
    long a, b;
    if (!f) return;
    while (fgets(line, sizeof(line), f)) {
        if (sscanf(line, "pos %ld %ld", &a, &b) == 2)  { g_posX = (int)a; g_posY = (int)b; continue; }
        if (sscanf(line, "topmost %ld", &a) == 1)      { g_topmost = (a != 0); continue; }
        if (sscanf(line, "opacity %ld", &a) == 1)      { g_opacityPct = (int)a; continue; }
        if (sscanf(line, "interval %ld", &a) == 1)     { g_intervalDdj = (int)a; continue; }
        if (sscanf(line, "minimal %ld", &a) == 1)      { g_minimal = (a != 0); continue; }
        if (sscanf(line, "zoom %ld", &a) == 1)         { g_zoomPct = (int)a; continue; }
        if (sscanf(line, "hover %ld", &a) == 1)        { g_hoverMode = (a != 0); continue; }
        if (sscanf(line, "netunit %ld", &a) == 1)      { g_netUnit = (a >= 0 && a <= 2) ? (int)a : 1; continue; }
        if (sscanf(line, "shape %ld", &a) == 1)        { g_shape = (a >= 0 && a <= 2) ? (int)a : 0; continue; }
        if (sscanf(line, "vert %ld", &a) == 1)         { g_vert = (a >= 0 && a <= 2) ? (int)a : 0; continue; }
        if (sscanf(line, "bg %lx", &a) == 1)            { g_bgColor = RGB(a & 255, (a >> 8) & 255, (a >> 16) & 255); continue; }
        if (sscanf(line, "fg %lx", &a) == 1)            { g_fgColor = RGB(a & 255, (a >> 8) & 255, (a >> 16) & 255); continue; }
    }
    fclose(f);
}

static void saveConfig(void)
{
    FILE *f = openCfg("w");
    if (!f) return;
    fprintf(f, "pos %d %d\ntopmost %d\nopacity %d\ninterval %d\nminimal %d\nzoom %d\nhover %d\nnetunit %d\n"
               "shape %d\nvert %d\nbg %06lx\nfg %06lx\n",
            g_posX, g_posY, g_topmost ? 1 : 0, g_opacityPct, g_intervalDdj,
            g_minimal ? 1 : 0, g_zoomPct, g_hoverMode ? 1 : 0, g_netUnit,
            g_shape, g_vert, g_bgColor, g_fgColor);
    fclose(f);
}

/* ---------------- 颜色派生 ---------------- */

static COLORREF scaleColor(COLORREF c, double k)
{
    int r = (int)(GetRValue(c) * k + 0.5);
    int g = (int)(GetGValue(c) * k + 0.5);
    int b = (int)(GetBValue(c) * k + 0.5);
    if (r > 255) r = 255;
    if (g > 255) g = 255;
    if (b > 255) b = 255;
    return RGB(r, g, b);
}

static COLORREF mixColor(COLORREF a, COLORREF b, double t)
{
    return RGB((int)(GetRValue(a) + (GetRValue(b) - GetRValue(a)) * t + 0.5),
               (int)(GetGValue(a) + (GetGValue(b) - GetGValue(a)) * t + 0.5),
               (int)(GetBValue(a) + (GetBValue(b) - GetBValue(a)) * t + 0.5));
}

static void deriveColors(void)
{
    g_cTitle  = scaleColor(g_fgColor, 0.76);
    g_cName   = scaleColor(g_fgColor, 0.65);
    g_cSub    = scaleColor(g_fgColor, 0.45);
    g_cTrack  = mixColor(g_bgColor, g_fgColor, 0.10);
    g_cBorder = mixColor(g_bgColor, g_fgColor, 0.13);
}

/* ---------------- PDH ---------------- */

static BOOL addCounter(const WCHAR *path, PDH_HCOUNTER *out)
{
    return PdhAddEnglishCounterW(g_q, path, 0, out) == PDH_CSTATUS_VALID_DATA;
}

static BOOL initPdh(void)
{
    if (PdhOpenQuery(NULL, 0, &g_q) != PDH_CSTATUS_VALID_DATA) return FALSE;
    if (!addCounter(L"\\GPU Engine(*)\\Utilization Percentage", &g_gpu))     goto bad;
    if (!addCounter(L"\\Network Interface(*)\\Bytes Received/sec", &g_nr))    goto bad;
    if (!addCounter(L"\\Network Interface(*)\\Bytes Sent/sec", &g_ns))        goto bad;
    PdhCollectQueryData(g_q);      /* 建立差值基线，此次读数无意义 */
    return TRUE;
bad:
    PdhCloseQuery(g_q); g_q = NULL;
    return FALSE;
}

static BOOL statusOk(DWORD s) { return s == PDH_CSTATUS_VALID_DATA || s == PDH_CSTATUS_NEW_DATA; }

static double arraySum(PDH_HCOUNTER c)
{
    DWORD sz = 0, n = 0, i;
    FmtItem *it;
    double sum = 0.0;
    if ((DWORD)PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &sz, &n, NULL) != PDH_MORE_DATA) return 0.0;
    it = (FmtItem *)malloc(sz ? sz : 1);
    if (!it) return 0.0;
    if (PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &sz, &n, (PVOID)it) == ERROR_SUCCESS)
        for (i = 0; i < n; i++)
            if (statusOk(it[i].v.CStatus)) sum += it[i].v.doubleValue;
    free(it);
    return sum;
}

/* GPU：每个实例是"某进程占某引擎节点的比例"，所以按引擎节点（实例名去掉 pid_ 前缀，
 * 即从 luid_ 起的整段）跨进程累加，再取最忙的那个节点。
 * 不能按 engtype 平均：核显 3D 类型有十几个引擎节点，空闲节点会把结果稀释成零头。 */
static double gpuUsage(void)
{
    DWORD sz = 0, n = 0, i;
    FmtItem *it;
    struct { WCHAR node[64]; double total; } grp[128];
    int nGrp = 0, j;
    double best = 0.0;

    if ((DWORD)PdhGetFormattedCounterArrayW(g_gpu, PDH_FMT_DOUBLE, &sz, &n, NULL) != PDH_MORE_DATA)
        return 0.0;
    it = (FmtItem *)malloc(sz ? sz : 1);
    if (!it) return 0.0;
    memset(grp, 0, sizeof(grp));

    if (PdhGetFormattedCounterArrayW(g_gpu, PDH_FMT_DOUBLE, &sz, &n, (PVOID)it) == ERROR_SUCCESS) {
        for (i = 0; i < n; i++) {
            WCHAR *node = wcsstr(it[i].szName, L"luid_");
            int gi = -1;
            double v;
            if (!node || !statusOk(it[i].v.CStatus)) continue;
            if (wcslen(node) >= ARRAYSIZE(grp[0].node)) continue;
            v = it[i].v.doubleValue; if (v < 0) v = 0;

            for (j = 0; j < nGrp; j++) if (wcscmp(grp[j].node, node) == 0) { gi = j; break; }
            if (gi < 0) {
                if (nGrp >= (int)ARRAYSIZE(grp)) continue;
                gi = nGrp++;
                StringCchCopyW(grp[gi].node, ARRAYSIZE(grp[gi].node), node);
                grp[gi].total = 0.0;
            }
            grp[gi].total += v;
        }
    }
    free(it);

    for (j = 0; j < nGrp; j++) if (grp[j].total > best) best = grp[j].total;
    return best > 100.0 ? 100.0 : best;
}

/* ---------------- 采样 ---------------- */

static void sample(Metrics *m)
{
    FILETIME fi, fk, fu;
    ULARGE_INTEGER i, k, u;
    MEMORYSTATUSEX ms;

    memset(m, 0, sizeof(*m));
    memset(&i, 0, sizeof(i)); memset(&k, 0, sizeof(k)); memset(&u, 0, sizeof(u));
    ms.dwLength = sizeof(ms);

    if (GetSystemTimes(&fi, &fk, &fu)) {
        i.LowPart = fi.dwLowDateTime;  i.HighPart = fi.dwHighDateTime;
        k.LowPart = fk.dwLowDateTime;  k.HighPart = fk.dwHighDateTime;
        u.LowPart = fu.dwLowDateTime;  u.HighPart = fu.dwHighDateTime;
        if (g_haveTimes) {
            double didle = (double)(i.QuadPart - g_prevIdle.QuadPart);
            double tot   = (double)(k.QuadPart - g_prevKernel.QuadPart)
                         + (double)(u.QuadPart - g_prevUser.QuadPart);
            if (tot > 0.0) {
                double pct = (1.0 - didle / tot) * 100.0;
                m->cpu = pct < 0 ? 0 : (pct > 100 ? 100 : pct);
            }
        }
        g_prevIdle = i; g_prevKernel = k; g_prevUser = u; g_haveTimes = TRUE;
    }

    if (GlobalMemoryStatusEx(&ms)) {
        m->mem = (double)ms.dwMemoryLoad;
        m->memTotalGb = (double)ms.ullTotalPhys / 1073741824.0;
        m->memUsedGb  = (double)(ms.ullTotalPhys - ms.ullAvailPhys) / 1073741824.0;
    }

    if (g_pdhOk) {
        PdhCollectQueryData(g_q);
        m->gpu   = gpuUsage();
        m->netR  = arraySum(g_nr) / 1048576.0;
        m->netS  = arraySum(g_ns) / 1048576.0;
    }

    m->uptimeSec = (double)GetTickCount64() / 1000.0;
}

/* ---------------- 绘制 ---------------- */

static COLORREF accentFor(double pct)
{
    if (pct >= 85) return CLR_HOT;
    if (pct >= 60) return CLR_WARN;
    if (pct <= 1)  return CLR_IDLE;
    return CLR_OK;
}

static void roundRectFill(HDC dc, int x, int y, int w, int h, int rad, COLORREF fill)
{
    HBRUSH b = CreateSolidBrush(fill), ob;
    HPEN p = CreatePen(PS_SOLID, 1, fill), op;
    if (w <= 0 || h <= 0) { DeleteObject(b); DeleteObject(p); return; }
    ob = (HBRUSH)SelectObject(dc, b);
    op = (HPEN)SelectObject(dc, p);
    RoundRect(dc, x, y, x + w, y + h, rad * 2, rad * 2);
    SelectObject(dc, op); SelectObject(dc, ob);
    DeleteObject(b); DeleteObject(p);
}

static void roundRectStroke(HDC dc, int x, int y, int w, int h, int rad, COLORREF c)
{
    HPEN p = CreatePen(PS_SOLID, 1, c), op = (HPEN)SelectObject(dc, p);
    HBRUSH ob = (HBRUSH)SelectObject(dc, GetStockObject(NULL_BRUSH));
    RoundRect(dc, x, y, x + w, y + h, rad * 2, rad * 2);
    SelectObject(dc, op); SelectObject(dc, ob);
    DeleteObject(p);
}

static int drawRight(HDC dc, const WCHAR *s, int right, int top, int h, COLORREF c, HFONT f)
{
    SIZE sz; RECT r;
    HFONT of = (HFONT)SelectObject(dc, f);
    GetTextExtentPoint32W(dc, s, (int)wcslen(s), &sz);
    r.left = right - sz.cx; r.right = right; r.top = top; r.bottom = top + h;
    SetTextColor(dc, c);
    DrawTextW(dc, s, -1, &r, DT_RIGHT | DT_TOP | DT_NOPREFIX | DT_SINGLELINE);
    SelectObject(dc, of);
    return sz.cx;
}

static void drawLeft(HDC dc, const WCHAR *s, int left, int top, int h, COLORREF c, HFONT f)
{
    RECT r;
    HFONT of = (HFONT)SelectObject(dc, f);
    r.left = left; r.right = left + 8000; r.top = top; r.bottom = top + h;
    SetTextColor(dc, c);
    DrawTextW(dc, s, -1, &r, DT_LEFT | DT_TOP | DT_NOPREFIX | DT_SINGLELINE);
    SelectObject(dc, of);
}

/* 单位由用户在菜单里定：KB/s / MB/s / GB/s */
static void fmtRate(WCHAR *out, size_t cap, double mbps)
{
    switch (g_netUnit) {
    case 0:  StringCchPrintfW(out, cap, L"%.0f KB/s", mbps * 1024.0); break;
    case 2:  StringCchPrintfW(out, cap, L"%.2f GB/s", mbps / 1024.0); break;
    default: if (mbps >= 100.0) StringCchPrintfW(out, cap, L"%.0f MB/s", mbps);
             else               StringCchPrintfW(out, cap, L"%.2f MB/s", mbps);
    }
}

/* 极简模式单行空间紧，单位用单字母 */
static void fmtRateShort(WCHAR *out, size_t cap, double mbps)
{
    switch (g_netUnit) {
    case 0:  StringCchPrintfW(out, cap, L"%.0fK", mbps * 1024.0); break;
    case 2:  StringCchPrintfW(out, cap, L"%.2fG", mbps / 1024.0); break;
    default: if (mbps >= 100.0) StringCchPrintfW(out, cap, L"%.0fM", mbps);
             else               StringCchPrintfW(out, cap, L"%.2fM", mbps);
    }
}

static void fmtUptime(WCHAR *out, size_t cap, double s)
{
    int t = (int)s, d = t / 86400, h = (t % 86400) / 3600, m = (t % 3600) / 60;
    if (d) StringCchPrintfW(out, cap, L"%d天 %02d:%02d", d, h, m);
    else   StringCchPrintfW(out, cap, L"%02d:%02d", h, m);
}

/* 返回 DIB 的像素指针；*outBmp 由调用方负责 DeleteObject */
static void *drawCard(HDC ref, HBITMAP *outBmp)
{
    BITMAPINFO bi; HDC dc; HBITMAP bmp; HGDIOBJ old; void *bits = NULL;
    int w = g_cardW, h = g_cardH, y, lh, bh, rowH, rx, barW;
    WCHAR buf[96], tail[64], host[64], a[40], b2[40];
    DWORD hlen = ARRAYSIZE(host);

    *outBmp = NULL;
    memset(&bi, 0, sizeof(bi));
    bi.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    bi.bmiHeader.biWidth = w; bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1; bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;

    dc = CreateCompatibleDC(ref);
    bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &bits, NULL, 0);
    if (!bmp) { DeleteDC(dc); return NULL; }
    old = SelectObject(dc, bmp);
    SetBkMode(dc, TRANSPARENT);

    if (g_shape == 2) {
        /* 纯内容：不画背景/描边；画布未初始化，必须清零让空处保持全透明 */
        memset(bits, 0, (size_t)w * h * 4);
    } else {
        int rad = (g_shape == 0) ? px(RADIUS) : 0;
        roundRectFill(dc, 0, 0, w, h, rad, CLR_BG);
        roundRectStroke(dc, 0, 0, w - 1, h - 1, rad, CLR_BORDER);
    }

    lh = px(18); rx = w - px(PAD_X);

    if (g_minimal) {
        /* 极简模式：没有主机名/运行时长/进度条 */
        if (g_vert == 2) {
            /* 分列：名称、数值各占一行，下载、上传各占一行 */
            int ty = px(8), rh = px(16);
            if (g_mValid) {
                fmtRateShort(a, ARRAYSIZE(a), g_m.netR);
                fmtRateShort(b2, ARRAYSIZE(b2), g_m.netS);
                drawLeft(dc, L"CPU", px(PAD_X), ty, rh, CLR_NAME, g_uiFont); ty += rh;
                StringCchPrintfW(buf, ARRAYSIZE(buf), L"%.0f%%", g_m.cpu);
                drawLeft(dc, buf, px(PAD_X), ty, rh, CLR_VALUE, g_monoFont); ty += rh + px(5);
                drawLeft(dc, L"内存", px(PAD_X), ty, rh, CLR_NAME, g_uiFont); ty += rh;
                StringCchPrintfW(buf, ARRAYSIZE(buf), L"%.0f%%", g_m.mem);
                drawLeft(dc, buf, px(PAD_X), ty, rh, CLR_VALUE, g_monoFont); ty += rh + px(5);
                drawLeft(dc, L"GPU", px(PAD_X), ty, rh, CLR_NAME, g_uiFont); ty += rh;
                StringCchPrintfW(buf, ARRAYSIZE(buf), L"%.0f%%", g_m.gpu);
                drawLeft(dc, buf, px(PAD_X), ty, rh, CLR_VALUE, g_monoFont); ty += rh + px(5);
                StringCchPrintfW(buf, ARRAYSIZE(buf), L"↓%s", a);
                drawLeft(dc, buf, px(PAD_X), ty, rh, CLR_SUB, g_monoFont); ty += rh + px(2);
                StringCchPrintfW(buf, ARRAYSIZE(buf), L"↑%s", b2);
                drawLeft(dc, buf, px(PAD_X), ty, rh, CLR_SUB, g_monoFont);
            } else {
                drawLeft(dc, L"初始化…", px(PAD_X), ty, rh, CLR_NAME, g_uiFont);
            }
        } else if (g_vert == 1) {
            /* 单列：四项各占一行，左对齐 */
            int ty = px(9);
            if (g_mValid) {
                fmtRateShort(a, ARRAYSIZE(a), g_m.netR);
                fmtRateShort(b2, ARRAYSIZE(b2), g_m.netS);
                StringCchPrintfW(buf, ARRAYSIZE(buf), L"CPU %.0f%%", g_m.cpu);
                drawLeft(dc, buf, px(PAD_X), ty, lh, CLR_VALUE, g_uiFont); ty += lh + px(3);
                StringCchPrintfW(buf, ARRAYSIZE(buf), L"内存 %.0f%%", g_m.mem);
                drawLeft(dc, buf, px(PAD_X), ty, lh, CLR_VALUE, g_uiFont); ty += lh + px(3);
                StringCchPrintfW(buf, ARRAYSIZE(buf), L"GPU %.0f%%", g_m.gpu);
                drawLeft(dc, buf, px(PAD_X), ty, lh, CLR_VALUE, g_uiFont); ty += lh + px(3);
                StringCchPrintfW(buf, ARRAYSIZE(buf), L"↓%s ↑%s", a, b2);
                drawLeft(dc, buf, px(PAD_X), ty, lh, CLR_SUB, g_monoFont);
            } else {
                drawLeft(dc, L"初始化…", px(PAD_X), ty, lh, CLR_NAME, g_uiFont);
            }
        } else {
            /* 单行显示四项指标 */
            int ty = (h - lh) / 2;
            if (g_mValid) {
            RECT r; HFONT of; int rw;
            fmtRateShort(a, ARRAYSIZE(a), g_m.netR);
            fmtRateShort(b2, ARRAYSIZE(b2), g_m.netS);
            StringCchPrintfW(buf, ARRAYSIZE(buf), L"↓%s ↑%s", a, b2);
            rw = drawRight(dc, buf, rx, ty, lh, CLR_SUB, g_monoFont);
            StringCchPrintfW(buf, ARRAYSIZE(buf), L"CPU %.0f%% · 内存 %.0f%% · GPU %.0f%%",
                             g_m.cpu, g_m.mem, g_m.gpu);
            /* 左侧文字裁切到网速区之前，极长时以省略号收尾 */
            of = (HFONT)SelectObject(dc, g_uiFont);
            SetTextColor(dc, CLR_VALUE);
            r.left = px(PAD_X); r.right = rx - rw - px(8);
            r.top = ty; r.bottom = ty + lh;
            DrawTextW(dc, buf, -1, &r, DT_LEFT | DT_TOP | DT_NOPREFIX | DT_SINGLELINE | DT_END_ELLIPSIS);
            SelectObject(dc, of);
            } else {
                drawLeft(dc, L"初始化…", px(PAD_X), ty, lh, CLR_NAME, g_uiFont);
            }
        }
    } else {
        if (!GetComputerNameW(host, &hlen)) StringCchCopyW(host, ARRAYSIZE(host), L"PC");

        y = px(12);
        barW = w - px(PAD_X) * 2;
        drawLeft(dc, host, px(PAD_X), y, lh, CLR_TITLE, g_uiFont);
        if (g_mValid) {
            /* 不显示 CPU 频率：HKLM 的 ~MHz 实测恒为 2496、不随负载变化，
             * 那是标称值而非实时频率，显示出来只会误导。 */
            fmtUptime(tail, ARRAYSIZE(tail), g_m.uptimeSec);
            StringCchPrintfW(buf, ARRAYSIZE(buf), L"已运行 %s", tail);
            drawRight(dc, buf, rx, y + px(2), lh, CLR_SUB, g_uiFont);
        }
        y += lh + px(9);
        bh = px(6); rowH = px(17);

#define ROW_PCT(label, val)                                                    \
    do {                                                                       \
        StringCchPrintfW(buf, ARRAYSIZE(buf), L"%.0f%%", (val));               \
        drawLeft(dc, (label), px(PAD_X), y, rowH, CLR_NAME, g_uiFont);         \
        drawRight(dc, buf, rx, y, rowH, CLR_VALUE, g_monoFont);                \
        y += rowH + px(3);                                                     \
        roundRectFill(dc, px(PAD_X), y, barW, bh, bh / 2, CLR_TRACK);          \
        { int fill = (int)(barW * ((val) / 100.0) + 0.5);                      \
          if (fill > 0) roundRectFill(dc, px(PAD_X), y, fill, bh, bh / 2,      \
                                      accentFor(val)); }                       \
        y += bh + px(9);                                                       \
    } while (0)

        if (g_mValid) {
            ROW_PCT(L"CPU",  g_m.cpu);
            StringCchPrintfW(buf, ARRAYSIZE(buf), L"%.0f%% · %.1f/%.0fG",
                             g_m.mem, g_m.memUsedGb, g_m.memTotalGb);
            drawLeft(dc, L"内存", px(PAD_X), y, rowH, CLR_NAME, g_uiFont);
            drawRight(dc, buf, rx, y, rowH, CLR_VALUE, g_monoFont);
            y += rowH + px(3);
            roundRectFill(dc, px(PAD_X), y, barW, bh, bh / 2, CLR_TRACK);
            { int fill = (int)(barW * (g_m.mem / 100.0) + 0.5);
              if (fill > 0) roundRectFill(dc, px(PAD_X), y, fill, bh, bh / 2, accentFor(g_m.mem)); }
            y += bh + px(9);

            ROW_PCT(L"GPU", g_m.gpu);

            fmtRate(a, ARRAYSIZE(a), g_m.netR);
            fmtRate(b2, ARRAYSIZE(b2), g_m.netS);
            StringCchPrintfW(buf, ARRAYSIZE(buf), L"↓%s ↑%s", a, b2);
            drawLeft(dc, L"网络", px(PAD_X), y, rowH, CLR_NAME, g_uiFont);
            drawRight(dc, buf, rx, y, rowH, CLR_VALUE, g_monoFont);
        } else {
            drawLeft(dc, L"初始化…", px(PAD_X), y, rowH, CLR_NAME, g_uiFont);
        }
#undef ROW_PCT
    }

    SelectObject(dc, old);
    DeleteDC(dc);
    *outBmp = bmp;
    return bits;
}

/* 2 倍尺寸画一个实心圆角矩形，下采样成逐像素覆盖率（0..255） */
static BYTE *buildCoverage(HDC ref)
{
    BITMAPINFO bi; HDC dc; HBITMAP bmp; HGDIOBJ old; void *bits = NULL;
    BYTE *cov;
    int w = g_cardW, h = g_cardH, W = w * 2, H = h * 2, x, y;
    (void)H;

    memset(&bi, 0, sizeof(bi));
    bi.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    bi.bmiHeader.biWidth = W; bi.bmiHeader.biHeight = -H;
    bi.bmiHeader.biPlanes = 1; bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;

    dc = CreateCompatibleDC(ref);
    bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &bits, NULL, 0);
    if (!bmp) { DeleteDC(dc); return NULL; }
    old = SelectObject(dc, bmp);
    roundRectFill(dc, 0, 0, W, H, (g_shape == 0) ? px(RADIUS) * 2 : 0, RGB(255, 255, 255));

    cov = (BYTE *)malloc((size_t)w * h);
    if (cov) {
        for (y = 0; y < h; y++) {
            BYTE *r0 = (BYTE *)bits + (size_t)(y * 2)     * W * 4;
            BYTE *r1 = (BYTE *)bits + (size_t)(y * 2 + 1) * W * 4;
            BYTE *dst = cov + (size_t)y * w;
            for (x = 0; x < w; x++) {
                int s = r0[x * 8] + r0[x * 8 + 4] + r1[x * 8] + r1[x * 8 + 4];
                dst[x] = (BYTE)((s + 2) / 4);
            }
        }
    }
    SelectObject(dc, old);
    DeleteDC(dc);
    DeleteObject(bmp);
    return cov;
}

static void repaint(void)
{
    HDC screen = GetDC(g_hwnd ? g_hwnd : GetDesktopWindow());
    HBITMAP card = NULL; BYTE *cov; void *bits;
    POINT srcPt = {0, 0}, dstPt;
    SIZE sz; BLENDFUNCTION bf;
    int i, npix; HDC memdc; HGDIOBJ old;
    /* 启动后先点亮一段时间，避免"悬停显示"状态下重启后找不到卡片；
     * 菜单/颜色对话框打开期间不进入隐藏，避免卡片在用户眼前变幽灵 */
    int ghost = g_hoverMode && !g_hovering && !g_modalHold && GetTickCount() > g_revealUntil;

    bits = drawCard(screen, &card);
    cov = buildCoverage(screen);
    ReleaseDC(g_hwnd, screen);

    if (!bits || !cov) {
        if (card) DeleteObject(card);
        if (cov) free(cov);
        return;
    }

    npix = g_cardW * g_cardH;
    for (i = 0; i < npix; i++) {
        BYTE *p = (BYTE *)bits + (size_t)i * 4;
        int a;
        if (g_shape == 2) {
            /* 纯内容：背景像素 RGB=0 → alpha=0 全透明；内容像素按最亮通道定不透明度
             * （GDI 画不出逐像素 alpha，用亮度近似覆盖率；×1.4 补偿彩色进度条的暗通道）。
             * 空隙给 1/255 的不可见垫层：alpha=0 会被分层窗口点击穿透，
             * 整卡都按不上面/右键唤不出菜单；1/255 混合只有 0.4%，肉眼不可见 */
            int m = p[0] > p[1] ? (p[0] > p[2] ? p[0] : p[2]) : (p[1] > p[2] ? p[1] : p[2]);
            a = m * 7 / 5;
            if (a > 255) a = 255;
            if (!a) a = 1;
        } else {
            a = (cov[i] * g_opacityPct) / 100;
        }
        /* 悬停显示的隐藏态：alpha=0 的像素会被分层窗口点击穿透、再也收不到悬停，
         * 但命中只要求非零、不要求可见——实测 alpha≈3/255 肉眼已不可见且悬停照常触发 */
        if (ghost && a) a = 3;
        p[3] = (BYTE)a;
        p[0] = (BYTE)((p[0] * a) / 255);
        p[1] = (BYTE)((p[1] * a) / 255);
        p[2] = (BYTE)((p[2] * a) / 255);
    }

    memdc = CreateCompatibleDC(NULL);
    old = SelectObject(memdc, card);
    dstPt.x = g_posX; dstPt.y = g_posY;
    sz.cx = g_cardW; sz.cy = g_cardH;
    bf.BlendOp = AC_SRC_OVER; bf.BlendFlags = 0;
    bf.SourceConstantAlpha = 255; bf.AlphaFormat = AC_SRC_ALPHA;
    UpdateLayeredWindow(g_hwnd, NULL, &dstPt, &sz, memdc, &srcPt, 0, &bf, ULW_ALPHA);
    SelectObject(memdc, old);
    DeleteDC(memdc);

    DeleteObject(card);
    free(cov);
}

/* ---------------- 交互 ---------------- */

static void onCommand(WORD id);
static void rebuildFonts(void);

static void showMenu(void)
{
    HMENU m = CreatePopupMenu(), sub;
    int i, cmd; WCHAR t[32];

    AppendMenuW(m, MF_STRING | (g_topmost ? MF_CHECKED : 0), IDM_TOPMOST, L"窗口置顶");
    AppendMenuW(m, MF_STRING | (g_minimal ? MF_CHECKED : 0), IDM_MINIMAL, L"极简模式");
    AppendMenuW(m, MF_STRING | (g_hoverMode ? MF_CHECKED : 0), IDM_HOVER, L"悬停显示");

    sub = CreatePopupMenu();
    {
        static const WCHAR *vt[3] = { L"单行", L"单列", L"分列" };
        for (i = 0; i < 3; i++)
            AppendMenuW(sub, MF_STRING | (g_vert == i ? MF_CHECKED : 0), IDM_VERT(i), vt[i]);
    }
    AppendMenuW(m, MF_POPUP, (UINT_PTR)sub, L"竖排(极简)");

    sub = CreatePopupMenu();
    for (i = 0; i < 10; i++) {
        StringCchPrintfW(t, ARRAYSIZE(t), L"%d%%", OPACITY_PCT[i]);
        AppendMenuW(sub, MF_STRING | (g_opacityPct == OPACITY_PCT[i] ? MF_CHECKED : 0),
                    IDM_OP(i), t);
    }
    AppendMenuW(m, MF_POPUP, (UINT_PTR)sub, L"透明度");

    sub = CreatePopupMenu();
    for (i = 0; i < 3; i++) {
        StringCchPrintfW(t, ARRAYSIZE(t), L"%.1f 秒", INTERVAL_DDJ[i] / 10.0);
        AppendMenuW(sub, MF_STRING | (g_intervalDdj == INTERVAL_DDJ[i] ? MF_CHECKED : 0),
                    IDM_IV(i), t);
    }
    AppendMenuW(m, MF_POPUP, (UINT_PTR)sub, L"刷新间隔");

    sub = CreatePopupMenu();
    for (i = 0; i < 5; i++) {
        StringCchPrintfW(t, ARRAYSIZE(t), L"%d%%", ZOOM_PCT[i]);
        AppendMenuW(sub, MF_STRING | (g_zoomPct == ZOOM_PCT[i] ? MF_CHECKED : 0),
                    IDM_ZOOM(i), t);
    }
    AppendMenuW(m, MF_POPUP, (UINT_PTR)sub, L"大小");

    sub = CreatePopupMenu();
    {
        static const WCHAR *un[3] = { L"KB/s", L"MB/s", L"GB/s" };
        for (i = 0; i < 3; i++)
            AppendMenuW(sub, MF_STRING | (g_netUnit == i ? MF_CHECKED : 0), IDM_UNIT(i), un[i]);
    }
    AppendMenuW(m, MF_POPUP, (UINT_PTR)sub, L"网速单位");

    sub = CreatePopupMenu();
    {
        static const WCHAR *sn[3] = { L"圆角", L"直角", L"纯内容" };
        for (i = 0; i < 3; i++)
            AppendMenuW(sub, MF_STRING | (g_shape == i ? MF_CHECKED : 0), IDM_SHAPE(i), sn[i]);
    }
    AppendMenuW(m, MF_POPUP, (UINT_PTR)sub, L"形状");

    sub = CreatePopupMenu();
    AppendMenuW(sub, MF_STRING, IDM_BGCLR, L"背景色…");
    AppendMenuW(sub, MF_STRING, IDM_FGCLR, L"文字色…");
    AppendMenuW(sub, MF_SEPARATOR, 0, NULL);
    AppendMenuW(sub, MF_STRING, IDM_CLRDEF, L"恢复默认配色");
    AppendMenuW(m, MF_POPUP, (UINT_PTR)sub, L"颜色");

    AppendMenuW(m, MF_SEPARATOR, 0, NULL);
    AppendMenuW(m, MF_STRING, IDM_QUIT, L"退出");

    {
        POINT p;
        GetCursorPos(&p);
        SetForegroundWindow(g_hwnd);
        g_modalHold = TRUE;    /* 模态期间暂停置顶保持与悬停隐藏（WM_TIMER 照常到达） */
        cmd = TrackPopupMenuEx(m, TPM_RIGHTBUTTON | TPM_RETURNCMD, p.x, p.y, g_hwnd, NULL);
        g_modalHold = FALSE;
        SendMessageW(g_hwnd, WM_NULL, 0, 0);   /* 交还焦点，避免菜单残留遮挡其它窗口 */
        if (cmd) onCommand((WORD)cmd);
    }
    DestroyMenu(m);
}

static void pickColor(COLORREF *c)
{
    CHOOSECOLORW cc; static COLORREF cust[16];
    memset(&cc, 0, sizeof(cc));
    cc.lStructSize = sizeof(cc);
    cc.hwndOwner = g_hwnd;
    cc.lpCustColors = cust;
    cc.rgbResult = *c;
    cc.Flags = CC_FULLOPEN | CC_RGBINIT;
    g_modalHold = TRUE;    /* 模态期间暂停置顶保持，避免卡片盖住颜色对话框 */
    if (ChooseColorW(&cc)) {
        *c = cc.rgbResult;
        deriveColors(); repaint(); saveConfig();
    }
    g_modalHold = FALSE;
}

static void onCommand(WORD id)
{
    if (id == IDM_TOPMOST) {
        g_topmost = !g_topmost;
        SetWindowPos(g_hwnd, g_topmost ? HWND_TOPMOST : HWND_NOTOPMOST,
                     0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        saveConfig();
    } else if (id >= IDM_OP(0) && id <= IDM_OP(9)) {
        g_opacityPct = OPACITY_PCT[id - IDM_OP(0)];
        saveConfig(); repaint();
    } else if (id == IDM_MINIMAL) {
        g_minimal = !g_minimal;
        refreshLayout(); rebuildFonts(); repaint();
        saveConfig();
    } else if (id == IDM_HOVER) {
        g_hoverMode = !g_hoverMode;
        repaint();
        saveConfig();
    } else if (id >= IDM_VERT(0) && id <= IDM_VERT(2)) {
        g_vert = id - IDM_VERT(0);
        refreshLayout(); repaint();
        saveConfig();
    } else if (id >= IDM_SHAPE(0) && id <= IDM_SHAPE(2)) {
        g_shape = id - IDM_SHAPE(0);
        repaint();
        saveConfig();
    } else if (id == IDM_BGCLR) {
        pickColor(&g_bgColor);
    } else if (id == IDM_FGCLR) {
        pickColor(&g_fgColor);
    } else if (id == IDM_CLRDEF) {
        g_bgColor = RGB(20, 22, 27);
        g_fgColor = RGB(238, 241, 244);
        deriveColors(); repaint(); saveConfig();
    } else if (id >= IDM_ZOOM(0) && id <= IDM_ZOOM(4)) {
        g_zoomPct = ZOOM_PCT[id - IDM_ZOOM(0)];
        refreshLayout(); rebuildFonts(); repaint();
        saveConfig();
    } else if (id >= IDM_UNIT(0) && id <= IDM_UNIT(2)) {
        g_netUnit = id - IDM_UNIT(0);
        saveConfig(); repaint();
    } else if (id >= IDM_IV(0) && id <= IDM_IV(2)) {
        g_intervalDdj = INTERVAL_DDJ[id - IDM_IV(0)];
        SetTimer(g_hwnd, 1, (UINT)(g_intervalDdj * 100), NULL);
        saveConfig();
    } else if (id == IDM_QUIT) {
        DestroyWindow(g_hwnd);
    }
}

static LRESULT CALLBACK WndProc(HWND h, UINT msg, WPARAM wp, LPARAM lp)
{
    switch (msg) {
    case WM_TIMER:
        if (wp == 1) {
            sample(&g_m); g_mValid = TRUE; repaint();
            /* topmost 组内"后声明者居上"，任务栏会不定期重新声明 z 序，
             * 把卡片压到卡片上面（表现为放任务栏处一会儿就被盖住"消失"）；
             * 每 tick 重新声明一次，开销可忽略。
             * 菜单/对话框模态期间必须暂停——模态循环照常分发 timer，
             * 若继续声明会把卡片提到刚弹出的菜单/对话框上面把它盖住 */
            if (g_topmost && !g_modalHold)
                SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0,
                             SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        return 0;
    case WM_LBUTTONDOWN:
        g_dragging = TRUE; g_dragMoved = FALSE;
        SetCapture(h);
        GetCursorPos(&g_dragCursor0);
        { RECT r; GetWindowRect(h, &r); g_dragWin0.x = r.left; g_dragWin0.y = r.top; }
        return 0;
    case WM_MOUSEMOVE:
        if (g_hoverMode && !g_hovering) {
            /* 悬停显示：进入卡片点亮。TME_LEAVE 是一次性的，
             * 每次 WM_MOUSELEAVE 后要靠下一条 mousemove 重新挂上 */
            TRACKMOUSEEVENT tme = { sizeof(tme), TME_LEAVE, h, 0 };
            g_hovering = TRUE;
            TrackMouseEvent(&tme);
            repaint();
        }
        if (g_dragging) {
            POINT cur;
            GetCursorPos(&cur);
            /* 必须用屏幕坐标求差：lParam 是客户区坐标，窗口一挪它的原点就跟着挪，
             * 再减去固定起点会自我抵消，表现为拖动跟不上鼠标、位移偏短 */
            g_posX = g_dragWin0.x + (cur.x - g_dragCursor0.x);
            g_posY = g_dragWin0.y + (cur.y - g_dragCursor0.y);
            if (cur.x != g_dragCursor0.x || cur.y != g_dragCursor0.y) g_dragMoved = TRUE;
            /* 分层窗口内容由上次 UpdateLayeredWindow 提供，单纯挪位置不需要重画；
             * 在每条 mousemove 上重建位图和蒙版会把帧率打烂 */
            SetWindowPos(h, NULL, g_posX, g_posY, 0, 0,
                         SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        return 0;
    case WM_LBUTTONUP:
        if (g_dragging) { g_dragging = FALSE; ReleaseCapture(); if (g_dragMoved) saveConfig(); }
        return 0;
    case WM_MOUSELEAVE:
        if (g_hovering) { g_hovering = FALSE; repaint(); }
        return 0;
    case WM_RBUTTONDOWN:
        showMenu();
        return 0;
    case WM_COMMAND:
        onCommand(LOWORD(wp));
        return 0;
    case WM_CLOSE:
        DestroyWindow(h);
        return 0;
    case WM_DESTROY:
        KillTimer(h, 1);
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcW(h, msg, wp, lp);
}

/* ---------------- 启动 ---------------- */

static void applyDpiAwareness(void)
{
    typedef HANDLE (WINAPI *SetThreadCtxFn)(HANDLE);
    typedef BOOL (WINAPI *SetCtxFn)(HANDLE);
    typedef UINT (WINAPI *GetDpiSysFn)(void);
    typedef HRESULT (WINAPI *GetDpiMonFn)(HMONITOR, int, UINT *, UINT *);
    typedef HMONITOR (WINAPI *MonFromPtFn)(POINT, DWORD);
    HMODULE u = GetModuleHandleW(L"user32.dll");
    HMODULE sh = LoadLibraryW(L"shcore.dll");
    SetThreadCtxFn stc = (SetThreadCtxFn)GetProcAddress(u, "SetThreadDpiAwarenessContext");
    SetCtxFn fn = (SetCtxFn)GetProcAddress(u, "SetProcessDpiAwarenessContext");
    GetDpiSysFn gd = (GetDpiSysFn)GetProcAddress(u, "GetDpiForSystem");
    GetDpiMonFn gm = sh ? (GetDpiMonFn)GetProcAddress(sh, "GetDpiForMonitor") : NULL;
    MonFromPtFn mp = (MonFromPtFn)GetProcAddress(u, "MonitorFromPoint");
    int dpi = 0; HDC dc;

    if (stc) stc((HANDLE)-4);           /* DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 */
    if (fn) fn((HANDLE)-4);
    else if (!stc) SetProcessDPIAware();

    /* GetDpiForSystem 对 PMv2 进程实测恒报 96；用 GetDpiForMonitor 取主屏真实 DPI，
     * 它不受感知上下文影响（Win8.1 起可用），失败再逐级回退 */
    if (gm && mp) {
        POINT p = {0, 0};
        UINT x = 0, y = 0;
        HMONITOR mon = mp(p, 2 /* MONITOR_DEFAULTTOPRIMARY */);
        if (mon && gm(mon, 0 /* MDT_EFFECTIVE_DPI */, &x, &y) == 0 && x > 0) dpi = (int)x;
    }
    if (dpi <= 0 && gd) dpi = (int)gd();
    if (dpi <= 0) {
        dc = GetDC(NULL);
        dpi = GetDeviceCaps(dc, LOGPIXELSX);
        ReleaseDC(NULL, dc);
    }

    g_scale = dpi / 96.0;
    refreshLayout();
}

static void makeFonts(void)
{
    g_uiFont = CreateFontW(-px(12), 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE,
                           DEFAULT_CHARSET, OUT_TT_PRECIS, CLIP_DEFAULT_PRECIS,
                           CLEARTYPE_QUALITY, DEFAULT_PITCH | FF_DONTCARE,
                           L"Microsoft YaHei UI");
    g_monoFont = CreateFontW(-px(12), 0, 0, 0, FW_SEMIBOLD, FALSE, FALSE, FALSE,
                             DEFAULT_CHARSET, OUT_TT_PRECIS, CLIP_DEFAULT_PRECIS,
                             CLEARTYPE_QUALITY, DEFAULT_PITCH | FF_DONTCARE,
                             L"Cascadia Mono");
}

/* 字号经 px() 依赖档位/模式，切换后要重建；绘制处都是临时 Select，删旧字体安全 */
static void rebuildFonts(void)
{
    if (g_uiFont)   DeleteObject(g_uiFont);
    if (g_monoFont) DeleteObject(g_monoFont);
    g_uiFont = g_monoFont = NULL;
    makeFonts();
}

int WINAPI WinMain(HINSTANCE inst, HINSTANCE prev, LPSTR cmd, int show)
{
    WNDCLASSW wc; MSG msg; RECT wa;
    (void)prev; (void)cmd; (void)show;

    loadConfig();
    deriveColors();
    applyDpiAwareness();
    makeFonts();
    /* 悬停显示状态下重启也先点亮 10 秒，让用户定位到卡片 */
    g_revealUntil = GetTickCount() + 10000;

    if (g_posX == -99999 || g_posY == -99999) {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &wa, 0);
        g_posX = wa.right - g_cardW - px(MARGIN);
        g_posY = wa.top + px(MARGIN);
    }

    g_pdhOk = initPdh();
    sample(&g_m);                 /* 建基线，此时数值无意义 */
    g_mValid = FALSE;

    memset(&wc, 0, sizeof(wc));
    wc.lpfnWndProc = WndProc;
    wc.hInstance = inst;
    wc.hCursor = LoadCursor(NULL, IDC_ARROW);
    wc.lpszClassName = L"AAXSysPulse";
    if (!RegisterClassW(&wc)) return 1;

    g_hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW | (g_topmost ? WS_EX_TOPMOST : 0),
        wc.lpszClassName, L"AAXSysPulse", WS_POPUP,
        g_posX, g_posY, g_cardW, g_cardH, NULL, NULL, inst, NULL);
    if (!g_hwnd) return 1;

    repaint();
    ShowWindow(g_hwnd, SW_SHOWNOACTIVATE);
    SetTimer(g_hwnd, 1, (UINT)(g_intervalDdj * 100), NULL);

    while (GetMessageW(&msg, NULL, 0, 0) > 0) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    saveConfig();
    if (g_pdhOk && g_q) PdhCloseQuery(g_q);
    if (g_uiFont) DeleteObject(g_uiFont);
    if (g_monoFont) DeleteObject(g_monoFont);
    return 0;
}
