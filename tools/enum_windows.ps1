# 枚举某进程所有顶层窗口（hwnd/title/class/rect）
param([int]$TargetPid = 0)
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class EW2 {
    [DllImport("user32.dll")] public static extern bool EnumWindows(Cb c, IntPtr p);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder b, int n);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int n);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    public delegate bool Cb(IntPtr h, IntPtr p);
    public struct RECT { public int L,T,R,B; }
}
"@
if ($TargetPid -le 0) {
    $pr = Get-Process AAXSysPulse -ErrorAction SilentlyContinue | Select-Object -First 1
    $TargetPid = [int]$pr.Id
}
Write-Output ("PID=$TargetPid")
$script:TargetPid = $TargetPid
$cb = [EW2+Cb]{
    param($h,$p)
    $opid = 0
    [void][EW2]::GetWindowThreadProcessId($h,[ref]$opid)
    $script:count = $script:count + 1
    if ($opid -eq $script:TargetPid) {
        $t=New-Object System.Text.StringBuilder 256
        $c=New-Object System.Text.StringBuilder 256
        [void][EW2]::GetWindowTextW($h,$t,256)
        [void][EW2]::GetClassNameW($h,$c,256)
        $r=New-Object EW2+RECT
        [void][EW2]::GetWindowRect($h,[ref]$r)
        [void]$script:list.Add(("hwnd=0x{0:X} pid={1} vis={2} class={3} rect=({4},{5})-({6},{7}) title='{8}'" -f $h.ToInt64(), $opid, [EW2]::IsWindowVisible($h), $c.ToString(), $r.L,$r.T,$r.R,$r.B, $t.ToString()))
    }
    return $true
}
$script:list = New-Object System.Collections.ArrayList
$script:count = 0
[void][EW2]::EnumWindows($cb,[IntPtr]::Zero)
Write-Output ("TOTAL=$script:count")
$script:list | ForEach-Object { Write-Output $_ }
Start-Sleep -Milliseconds 200
# keep delegate alive
[GC]::KeepAlive($cb)