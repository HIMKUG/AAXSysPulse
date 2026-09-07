# 打开设置窗口：合成托盘右键 -> 点击菜单「设置」
$src = @'
using System;
using System.Runtime.InteropServices;
public class Open2 {
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, UIntPtr e);
}
'@
Add-Type -TypeDefinition $src
$ovh = Get-Content "d:\code\sys-overlay-c-master\tools\ovhwnd.txt" -ErrorAction SilentlyContinue
if (-not $ovh) { $ovh = "0x560B6C" }
$h = [IntPtr]::new([Convert]::ToInt64($ovh,16))
Write-Output ("overlay hwnd=$ovh")
$cx=595; $cy=85
[void][Open2]::SetCursorPos($cx,$cy); Start-Sleep -Milliseconds 200
[void][Open2]::PostMessageW($h, 0x8001, [IntPtr]0, [IntPtr]0x205)
Start-Sleep -Milliseconds 500
[void][Open2]::SetCursorPos($cx+42, $cy+16); Start-Sleep -Milliseconds 150
[Open2]::mouse_event(0x0002,0,0,0,[UIntPtr]::Zero)
[Open2]::mouse_event(0x0004,0,0,0,[UIntPtr]::Zero)
Write-Output "sent open"