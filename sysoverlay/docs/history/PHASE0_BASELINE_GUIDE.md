# PHASE0_BASELINE_GUIDE.md - 基线数据采集指南

> Zig 编译器未在预期路径 `E:\dev\zig\zig.exe` 找到，本指南说明如何在有 Zig 的环境下完成 10 分钟运行数据采集。

---

## 前置条件

1. **安装 Zig 0.11+**
   - 下载地址：https://ziglang.org/download/
   - 推荐解压到 `E:\dev\zig\`（与 `build.bat` 默认路径一致）
   - 或修改 `build.bat` 第 6 行的 `ZIG=` 指向你的实际路径

2. **验证 Zig 可用**
   ```cmd
   E:\dev\zig\zig.exe version
   # 应输出类似 0.13.0
   ```

---

## 编译步骤

```cmd
cd /d D:\code\sys-overlay-c-master
build.bat
```

成功后在目录生成 `AAXSysPulse.exe`。

---

## 数据采集方法

### 方法一：Debug 输出重定向（推荐，无需代码修改）

1. **编译 Debug 版**（加入行号信息便于定位）：
   ```cmd
   set ZIG=E:\dev\zig\zig.exe
   "%ZIG%" cc -g -std=c11 -Wall -Wextra -DUNICODE -D_UNICODE ^
       overlay.c -o AAXSysPulse.exe ^
       -lpdh -lgdi32 -luser32 -ladvapi32 -lkernel32 -lcomdlg32
   ```

2. **运行并捕获输出**（10 分钟，约 600 条采样，假设 1s 间隔）：
   ```cmd
   AAXSysPulse.exe > PHASE0_baseline_raw.txt 2>&1
   timeout /t 600 /nobreak >nul
   taskkill /f /im AAXSysPulse.exe
   ```

3. **后处理脚本**（Python 示例）：
   ```python
   import re, csv
   from datetime import datetime

   pattern = re.compile(
       r'ts=(\d+) cpu=([\d.]+) mem=([\d.]+) gpu=([\d.]+) '
       r'memUsed=([\d.]+) memTotal=([\d.]+) '
       r'netR=([\d.]+) netS=([\d.]+)'
   )
   with open('PHASE0_baseline_raw.txt') as f, open('PHASE0_baseline.csv', 'w', newline='') as out:
       w = csv.writer(out)
       w.writerow(['timestamp_ms','cpu_pct','mem_pct','mem_used_gb','mem_total_gb','gpu_pct','net_rx_bps','net_tx_bps'])
       for line in f:
           m = pattern.search(line)
           if m:
               ts,cpu,mem,gpu,mu,mt,nr,ns = m.groups()
               w.writerow([ts, cpu, mem, mu, mt, gpu,
                           float(nr)*1048576, float(ns)*1048576])  # MB/s→B/s
   ```

### 方法二：代码注入采集（精确控制）

在 `overlay.c` 的 `sample()` 函数末尾添加 CSV 输出（临时修改，采集后还原）：

```c
// === 临时采集代码，采集后删除 ===
static FILE *g_log = NULL;
static void openLog(void) {
    g_log = fopen("PHASE0_baseline.csv", "w");
    if (g_log) fprintf(g_log, "timestamp_ms,cpu_pct,mem_pct,mem_used_gb,mem_total_gb,gpu_pct,net_rx_bps,net_tx_bps\n");
}
static void closeLog(void) {
    if (g_log) fclose(g_log);
}
// 在 sample() 末尾添加（现有代码后）:
static void sample(Metrics *m)
{
    // ... 现有代码保持不变 ...

    // === 临时采集代码 ===
    if (!g_log) openLog();
    if (g_log) {
        DWORD now = GetTickCount();
        fprintf(g_log, "%lu,%.1f,%.1f,%.2f,%.2f,%.1f,%.0f,%.0f\n",
                now, m->cpu, m->mem, m->memUsedGb, m->memTotalGb, m->gpu,
                m->netR * 1048576.0, m->netS * 1048576.0);
        fflush(g_log);
    }
}
```

然后：
```cmd
build.bat
AAXSysPulse.exe
# 运行 10 分钟
# 自动输出到 PHASE0_baseline.csv
```

**采集完成后务必还原 `overlay.c`！**

---

## 预期 CSV 格式

```csv
timestamp_ms,cpu_pct,mem_pct,mem_used_gb,mem_total_gb,gpu_pct,net_rx_bps,net_tx_bps
1234567,12.3,45.6,7.23,15.9,23.4,1048576.0,524288.0
1235567,15.7,46.1,7.34,15.9,25.1,2097152.0,1048576.0
...
```

| 列名 | 单位 | 说明 |
|------|------|------|
| `timestamp_ms` | ms | 自开机起的毫秒数（`GetTickCount64()`） |
| `cpu_pct` | % | CPU 使用率，0~100 |
| `mem_pct` | % | 内存负载百分比 |
| `mem_used_gb` | GB | 已用物理内存 |
| `mem_total_gb` | GB | 总物理内存 |
| `gpu_pct` | % | GPU 利用率（最繁忙引擎节点） |
| `net_rx_bps` | B/s | 网络接收字节率（原始值，非 MB/s 转换） |
| `net_tx_bps` | B/s | 网络发送字节率 |

---

## 采集时长说明

- 默认刷新间隔 1 秒（`g_intervalDdj=10`）
- 10 分钟 = 600 次采样
- 如需更长采集时间，可临时修改 `INTERVAL_DDJ` 为更大值（如 50 = 5 秒间隔）

---

## 注意事项

1. **网络单位**：内部存储为 MB/s，输出时转换为 B/s 以保持一致性
2. **GPU 采集失败**：如果 PDH 初始化失败（无 GPU 监控权限），`g_pdhOk=FALSE`，gpu/net 字段为 0
3. **内存数据**：始终有效，不依赖 PDH

---

*本文档因 Zig 编译器不可用而创建，仅作为采集指南。实际数据采集需要在有 Zig 的环境下完成。*
