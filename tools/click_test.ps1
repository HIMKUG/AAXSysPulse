# 模拟点击设置窗口 CPU toggle，验证 UI 刷新链路与配置变更；用法: click_test.ps1 <hwnd>
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W2 {
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern int GetDpiForWindow(IntPtr h);
    public struct RECT { public int L,T,R,B; }
}
"@

$h = if ($args.Count -gt 0) { [IntPtr]::new([Convert]::ToInt64($args[0], 16)) } else { [IntPtr]::Zero }
if ($h -eq [IntPtr]::Zero) { Write-Output "usage: click_test.ps1 <hwnd-hex>"; exit 1 }

$dpi = [W2]::GetDpiForWindow($h)
$cr = New-Object W2+RECT
[W2]::GetClientRect($h, [ref]$cr) | Out-Null
$scale = $dpi / 96.0

$widget_r = (360 - 16 - 12) * $scale
$toggle_l = $widget_r - 42 * $scale
$toggle_cx = [int](($toggle_l + $widget_r) / 2)
$card_top = (14 + 16 + 4) * $scale
$toggle_cy = [int]($card_top + 8 * $scale + 15 * $scale)
if (($toggle_cx -lt 0) -or ($toggle_cx -gt $cr.R) -or ($toggle_cy -gt $cr.B)) { Write-Output "target out of client (w=$($cr.R) h=$($cr.B))"; exit 1 }

Write-Output ("dpi={0} client=({1},{2}) target=({3},{4})" -f $dpi, $cr.R, $cr.B, $toggle_cx, $toggle_cy)

$lparam = [IntPtr](($toggle_cy -shl 16) -bor ($toggle_cx -band 0xFFFF))
[W2]::SendMessage($h, 0x0201, [IntPtr]0x0001, $lparam) | Out-Null
Start-Sleep -Milliseconds 80
[W2]::SendMessage($h, 0x0202, [IntPtr]0, $lparam) | Out-Null
Write-Output "click sent"