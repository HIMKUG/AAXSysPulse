# 截图指定窗口区域；用法: shot.ps1 <hwnd-hex> <outfile.png>
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class S {
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    public struct RECT { public int L,T,R,B; }
}
"@
Add-Type -AssemblyName System.Drawing

$h = [IntPtr]::new([Convert]::ToInt64($args[0], 16))
$out = $args[1]
$r = New-Object S+RECT
if (-not [S]::GetWindowRect($h, [ref]$r)) { Write-Output "no rect"; exit 1 }
$w = $r.R - $r.L; $ht = $r.B - $r.T
Write-Output ("win rect=({0},{1})-({2},{3}) size={4}x{5}" -f $r.L,$r.T,$r.R,$r.B,$w,$ht)
$bmp = New-Object System.Drawing.Bitmap($w, $ht)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L, $r.T, 0, 0, (New-Object System.Drawing.Size($w, $ht)))
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "saved $out"