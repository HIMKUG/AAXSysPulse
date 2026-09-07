# PrintWindow 抓取窗口内容（即使被遮挡）；用法: pshot.ps1 <hwnd-hex> <outfile.png>
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class PS {
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    public struct RECT { public int L,T,R,B; }
}
"@
Add-Type -AssemblyName System.Drawing
$h = [IntPtr]::new([Convert]::ToInt64($args[0], 16))
$out = $args[1]
$r = New-Object PS+RECT
[PS]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.R - $r.L; $ht = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap($w, $ht)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
[PS]::PrintWindow($h, $hdc, 2) | Out-Null   # PW_RENDERFULLCONTENT=2
$g.ReleaseHdc($hdc)
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "saved $out ($w x $ht)"