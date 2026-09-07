# 依次点击设置窗口的 6 个导航 tab 并截图每一页；用法: cap_pages.ps1 <hwnd-hex> <outdir>
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class CP {
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern int GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, UIntPtr e);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    public struct RECT { public int L,T,R,B; }
}
"@
$h = [IntPtr]::new([Convert]::ToInt64($args[0],16))
$dir = $args[1]
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
$dpi = [CP]::GetDpiForWindow($h)
$sc = $dpi / 96.0
$wr = New-Object CP+RECT
[void][CP]::GetWindowRect($h,[ref]$wr)
$cl = New-Object CP+RECT
[void][CP]::GetClientRect($h,[ref]$cl)
# 逻辑布局常量（与 settings.rs 一致）
$MARGIN_X=16; $NAV_H=32; $NAV_GAP=8; $TB_H=30; $CLIENT_W=420
$navTopRel = ($TB_H + $NAV_GAP) * $sc
$navH = $NAV_H * $sc
# 标题栏在窗口客户区高度内，客户区左边界 = 窗口左 + (窗口宽-客户宽)/2
$border = [int](($wr.R - $wr.L - $cl.R) / 2)
$navTopScreen = $wr.T + $border + $navTopRel
$nav0x = $wr.L + $border + $MARGIN_X * $sc
$segW = ($CLIENT_W - 2*$MARGIN_X) * $sc / 6
Write-Output ("dpi={0} scale={1} navTopScreen={2} segW={3}" -f $dpi,$sc,$navTopScreen,$segW)
function Click([int]$sx,[int]$sy){
    [void][CP]::SetCursorPos($sx,$sy); Start-Sleep -Milliseconds 80
    [CP]::mouse_event(0x0002,0,0,0,[UIntPtr]::Zero)
    [CP]::mouse_event(0x0004,0,0,0,[UIntPtr]::Zero)
}
for ($i=0; $i -lt 6; $i++){
    $cx = [int]($nav0x + ($i+0.5)*$segW)
    $cy = [int]($navTopScreen + $navH/2)
    Click $cx $cy
    Start-Sleep -Milliseconds 500
    $out = Join-Path $dir ("page_{0}.png" -f $i)
    pwsh -NoProfile -ExecutionPolicy Bypass -File "d:\code\sys-overlay-c-master\tools\pshot.ps1" $args[0] $out | Out-Null
    Write-Output ("page_{0}: clicked=({1},{2}) saved" -f $i,$cx,$cy)
}