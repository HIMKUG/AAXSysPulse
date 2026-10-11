"""PDH 磁盘计数器探测：打印 PdhCollectQueryData 的真实错误码。

用来判定「磁盘读写行不显示」是 PDH 本身在 OSHI 环境失败，还是 Rust 侧句柄类型不匹配。
用法：python tools/pdh_probe.py
"""
import ctypes
import time
from ctypes import wintypes

pdh = ctypes.WinDLL("pdh.dll")

PDH_FMT_DOUBLE = 0x00000200


class PDH_FMT_COUNTERVALUE(ctypes.Structure):
    class _U(ctypes.Union):
        _fields_ = [
            ("longValue", ctypes.c_long),
            ("doubleValue", ctypes.c_double),
            ("largeValue", ctypes.c_longlong),
            ("AnsiStringValue", ctypes.c_char_p),
            ("WideStringValue", ctypes.c_wchar_p),
        ]

    _fields_ = [("CStatus", wintypes.DWORD), ("Anonymous", _U)]


_NAMES = {
    0x00000000: "ERROR_SUCCESS",
    0x800007D0: "PDH_CSTATUS_NO_MACHINE",
    0x800007D1: "PDH_CSTATUS_NO_INSTANCE",
    0x800007D2: "PDH_MORE_DATA",
    0x800007D3: "PDH_CSTATUS_ITEM_NOT_VALIDATED",
    0x800007D4: "PDH_RETRY",
    0x800007D5: "PDH_NO_DATA",
    0x800007D6: "PDH_CALC_NEGATIVE_DENOMINATOR",
    0x800007D7: "PDH_CALC_NEGATIVE_TIMEBASE",
    0x800007D8: "PDH_CALC_NEGATIVE_VALUE",
    0x800007DC: "PDH_CANNOT_SET_DEFAULT_REALTIME_DATASOURCE",
    0xC0000BB8: "PDH_CSTATUS_NO_OBJECT",
    0xC0000BB9: "PDH_CSTATUS_NO_COUNTER",
    0xC0000BBA: "PDH_CSTATUS_INVALID_DATA",
    0xC0000BBB: "PDH_MEMORY_ALLOCATION_FAILURE",
    0xC0000BBC: "PDH_INVALID_HANDLE",
    0xC0000BBD: "PDH_INVALID_ARGUMENT",
    0xC0000BBE: "PDH_FUNCTION_NOT_FOUND",
    0xC0000BC0: "PDH_INVALID_DATA",
    0xC0000BC4: "PDH_CSTATUS_BAD_COUNTERNAME",
    0xC0000BC7: "PDH_NO_COUNTERS",
    0xC0000BCB: "PDH_INVALID_PATH",
    0xC0000BCD: "PDH_ACCESS_DENIED",
    0xC0000BCE: "PDH_INVALID_DATASOURCE",
    0xC0000BD1: "PDH_DATA_SOURCE_IS_LOG_FILE",
    0x800007E6: "PDH_QUERY_PERF_DATA_TIMEOUT",
}


def err(code):
    return _NAMES.get(code & 0xFFFFFFFF, "?")


def hx(code):
    return f"0x{code & 0xFFFFFFFF:08X}"


h = ctypes.c_void_p()
r = pdh.PdhOpenQueryW(None, ctypes.c_size_t(0), ctypes.byref(h))
print(f"PdhOpenQueryW ret={hx(r)} ({err(r)}) handle={h.value}")

# PdhAddEnglishCounterW(counter=NULL, 0) 语义：只注册路径，句柄由 PDH 内部管理
paths = [
    r"\PhysicalDisk(_Total)\Disk Read Bytes/sec",
    r"\PhysicalDisk(_Total)\Disk Write Bytes/sec",
]
handles = []
for p in paths:
    c = ctypes.c_void_p()
    r = pdh.PdhAddEnglishCounterW(
        h, ctypes.c_wchar_p(p), ctypes.c_size_t(0), ctypes.byref(c)
    )
    print(f"PdhAddEnglishCounterW[{p}] ret={hx(r)} ({err(r)}) counter={c.value}")
    handles.append((p, c, r))

for i in range(4):
    r = pdh.PdhCollectQueryData(h)
    print(f"PdhCollectQueryData #{i} ret={hx(r)} ({err(r)})")
    for p, c, _ in handles:
        buf = PDH_FMT_COUNTERVALUE()
        rr = pdh.PdhGetFormattedCounterValue(
            c, PDH_FMT_DOUBLE, None, ctypes.byref(buf)
        )
        print(
            f"   get[{p.split(chr(92))[-1]}] ret={hx(rr)} "
            f"status={buf.CStatus} val={buf.Anonymous.doubleValue}"
        )
    time.sleep(1.2)

pdh.PdhCloseQuery(h)
