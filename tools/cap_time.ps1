param([string]$Out = "C:/Users/VOS_User/AppData/Local/Temp/ov_time.png")
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win3 {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    public struct RECT { public int L, T, R, B; }
}
"@
$pidTarget = (Get-Process sysoverlay-app -ErrorAction Stop).Id
$best = [IntPtr]::Zero; $bestArea = 0; $bestRect = New-Object Win3+RECT
$cb = [Win3+EnumProc]{
    param($h, $l)
    $pid2 = [uint32]0
    [Win3]::GetWindowThreadProcessId($h, [ref]$pid2) | Out-Null
    if ($pid2 -eq $pidTarget -and [Win3]::IsWindowVisible($h)) {
        $r = New-Object Win3+RECT
        [Win3]::GetWindowRect($h, [ref]$r) | Out-Null
        $area = ($r.R - $r.L) * ($r.B - $r.T)
        if ($area -gt $bestArea) { $script:bestArea = $area; $script:best = $h; $script:bestRect = $r }
    }
    $true
}
[Win3]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
if ($best -eq [IntPtr]::Zero -or $bestArea -le 0) { throw "no visible overlay window for pid $pidTarget" }
$w = $bestRect.R - $bestRect.L
$cropH = [Math]::Min(32, $bestRect.B - $bestRect.T)
$bmp = New-Object System.Drawing.Bitmap($w, $cropH)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($bestRect.L, $bestRect.T, 0, 0, (New-Object System.Drawing.Size($w, $cropH)))
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "saved $Out rect=($($bestRect.L),$($bestRect.T)) ${w}x$($bestRect.B - $bestRect.T)"
