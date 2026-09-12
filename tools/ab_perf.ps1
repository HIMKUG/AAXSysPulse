<#
ab_perf.ps1 —— 悬浮窗 A/B 测量一条命令（悬浮窗性能恢复计划 P4.2，验收门槛⑤）

完成：构建 -> 双跑 -> 解析统一日志 -> 输出 A/B 分位数对比表。
  A 组 = 默认（内容指纹跳过 + 脏区 Rect 上传）
  B 组 = SYSOVERLAY_NODIRTY=1（每帧经典整窗 UpdateLayeredWindow）

用法（在仓库根目录）：
  powershell -ExecutionPolicy Bypass -File tools\ab_perf.ps1 -DurationSec 60
  powershell -ExecutionPolicy Bypass -File tools\ab_perf.ps1 -DurationSec 120 -SkipBuild

注意：
  - 每组跑 -DurationSec 秒（建议 >=60：尾刺与跳帧率需要长窗才稳定）。
  - 跑前强制停止正在运行的 sysoverlay-app（测量不允许旧实例干扰），跑完不自动重启。
  - 统一日志为 %LOCALAPPDATA%\SysOverlayC\sysoverlay_YYYYMMDD_HHMM.log
    （logging.rs：每会话一个文件，行格式 `[hh:mm:ss.mmm] [perf] ...`）。
    解析取「本轮运行期间有写入」的最新会话文件；与本轮同分钟名的旧会话文件
    先备份（.ab_bak）再删除，脚本结束前还原。
  - 每组原始日志另存 tools\ab_perf_A.log / ab_perf_B.log 供复查。
#>
param(
    [int]$DurationSec = 60,
    [switch]$SkipBuild,
    [string]$Exe = ""
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $repo "sysoverlay\target\release\sysoverlay-app.exe" }
$localAppData = $env:LOCALAPPDATA
if (-not $localAppData) { $localAppData = $env:TEMP }
$logDir = Join-Path $localAppData "SysOverlayC"

# 统一日志会话文件（logging.rs）：每会话一个 sysoverlay_YYYYMMDD_HHMM.log。
# After：只看该时刻之后有写入的文件（区分本轮与历史会话），返回最新一个。
function Get-SessionLog {
    param([datetime]$After = [datetime]::MinValue)
    if (-not (Test-Path $logDir)) { return $null }
    Get-ChildItem -Path $logDir -Filter "sysoverlay_*.log" -File -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -ge $After } |
        Sort-Object LastWriteTime | Select-Object -Last 1
}

function Write-Section([string]$t) {
    Write-Host ""
    Write-Host $t
    Write-Host ("=" * 64)
}

# 分位数（与 perf.rs 聚合行同口径：round((n-1)*p) 索引取值）。
function Get-Quant {
    param([System.Collections.Generic.List[long]]$S, [double]$P)
    if (-not $S -or $S.Count -eq 0) { return [long]0 }
    $arr = $S.ToArray(); [Array]::Sort($arr)
    $idx = [int][Math]::Round(($arr.Length - 1) * $P)
    return $arr[$idx]
}

function Invoke-Round {
    param([string]$Name, [hashtable]$ExtraEnv)

    Get-Process sysoverlay-app -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 600

    # 与本轮同分钟名的旧会话文件：应用按文件名追加，不删会混入上一轮数据。
    # 备份后删除；其他分钟的历史会话文件一律不动。
    $curName = "sysoverlay_{0:yyyyMMdd}_{0:HHmm}.log" -f (Get-Date)
    $curPath = Join-Path $logDir $curName
    if (Test-Path $curPath) { Copy-Item $curPath "$curPath.ab_bak" -Force }
    Remove-Item $curPath -ErrorAction SilentlyContinue
    $runStart = Get-Date

    $env:SYSOVERLAY_PERF = "1"
    foreach ($k in $ExtraEnv.Keys) { Set-Item -Path "Env:$k" -Value $ExtraEnv[$k] }
    try {
        $p = Start-Process -FilePath $Exe -PassThru
        $envDesc = ($ExtraEnv.Keys | ForEach-Object { "$_=$($ExtraEnv[$_])" }) -join ' '
        Write-Host ("[ab] {0}: pid={1} env=[{2}] wait={3}s" -f $Name, $p.Id, $envDesc, $DurationSec)
        Start-Sleep -Seconds $DurationSec
        if (Get-Process -Id $p.Id -ErrorAction SilentlyContinue) { Stop-Process -Id $p.Id -Force }
        Start-Sleep -Milliseconds 600
    }
    finally {
        Remove-Item Env:SYSOVERLAY_PERF -ErrorAction SilentlyContinue
        foreach ($k in $ExtraEnv.Keys) { Remove-Item -Path "Env:$k" -ErrorAction SilentlyContinue }
    }

    $sessionLog = Get-SessionLog -After $runStart.AddSeconds(-2)
    if (-not $sessionLog) { throw "$Name 组没有产生会话日志（$logDir\sysoverlay_*.log）——确认 SYSOVERLAY_PERF 生效" }
    Copy-Item $sessionLog.FullName (Join-Path $PSScriptRoot "ab_perf_$Name.log") -Force

    # ── 解析：frame_render 逐帧样本 + section 段样本 + frames/skipped 聚合行 ──
    # 行格式：`[hh:mm:ss.mmm] [perf] 原字段`（见 logging.rs）。
    # section 标签可含空格（如 `seg_upload pct=51`），故用 (.+) 而非 (\S+)。
    $frm = New-Object System.Collections.Generic.List[long]
    $sections = @{}
    $aggN = 0; $aggSkip = [long]0; $aggFrames = [long]0
    foreach ($ln in (Get-Content $sessionLog.FullName)) {
        if ($ln -match '^\[[\d:.]+\] \[perf\] frames=(\d+)\s+skipped=(\d+)\s+skip_pct=(\d+)%\s+avg_us=(\d+)\s+p50_us=(\d+)\s+p99_us=(\d+)\s+max_us=(\d+)') {
            $aggN++
            if ($aggN -gt 1) {
                # 首个聚合窗含启动突发（DIB/D2D 冷启动），统计上剔除
                $aggFrames += [long]$Matches[1]
                $aggSkip += [long]$Matches[2]
            }
            continue
        }
        if ($ln -match '^\[[\d:.]+\] \[perf\] frame_render ms=\d+ us=(\d+)') { $frm.Add([long]$Matches[1]); continue }
        if ($ln -match '^\[[\d:.]+\] \[perf\] section (.+) us=(\d+)') {
            $lab = $Matches[1]
            if (-not $sections.ContainsKey($lab)) { $sections[$lab] = New-Object System.Collections.Generic.List[long] }
            $sections[$lab].Add([long]$Matches[2])
        }
    }

    [pscustomobject]@{
        Name    = $Name
        Frames  = $frm.Count
        SkipPct = if (($aggFrames + $aggSkip) -gt 0) { [math]::Round(100.0 * $aggSkip / ($aggFrames + $aggSkip), 1) } else { 0 }
        P50     = Get-Quant $frm 0.50
        P99     = Get-Quant $frm 0.99
        Max     = Get-Quant $frm 1.0
        Sections = $sections
    }
}

# ── 1/4 构建 ──
if (-not $SkipBuild) {
    Write-Section "[ab] 1/4 cargo build --release"
    Push-Location (Join-Path $repo "sysoverlay")
    try {
        & cargo build --release -p sysoverlay-app
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    }
    finally { Pop-Location }
}
if (-not (Test-Path $Exe)) { throw "exe 不存在：$Exe（先构建，或用 -Exe 指定）" }

# ── 2/4 A 组：默认脏区路径 ──
Write-Section "[ab] 2/4 A 组（默认：指纹跳过 + 脏区 Rect 上传）"
$a = Invoke-Round -Name "A" -ExtraEnv @{}

# ── 3/4 B 组：整窗上传 ──
Write-Section "[ab] 3/4 B 组（SYSOVERLAY_NODIRTY=1：整窗上传）"
$b = Invoke-Round -Name "B" -ExtraEnv @{ SYSOVERLAY_NODIRTY = "1" }

# ── 4/4 分位数表 ──
Write-Section "[ab] 4/4 A/B 分位数对比（us，skip_pct 为 %）"

function FmtRow([string]$label, $av, $bv) {
    if ($av -is [long] -and $bv -is [long]) {
        $d = $bv - $av
        Write-Host ("| {0} | {1} | {2} | {3:+#;-#;0} |" -f $label, $av, $bv, $d)
    }
    else {
        Write-Host ("| {0} | {1} | {2} |  |" -f $label, $av, $bv)
    }
}

Write-Host "| 指标 | A 默认 | B NODIRTY | Delta(B-A) |"
Write-Host "|---|---:|---:|---:|"
FmtRow "frames（渲染帧）" $a.Frames $b.Frames
FmtRow "skip_pct %" $a.SkipPct $b.SkipPct
FmtRow "p50_us" $a.P50 $b.P50
FmtRow "p99_us" $a.P99 $b.P99
FmtRow "max_us" $a.Max $b.Max

$labels = @($a.Sections.Keys) + @($b.Sections.Keys) | Sort-Object -Unique
foreach ($l in $labels) {
    $av = if ($a.Sections.ContainsKey($l)) { Get-Quant $a.Sections[$l] 0.50 } else { $null }
    $bv = if ($b.Sections.ContainsKey($l)) { Get-Quant $b.Sections[$l] 0.50 } else { $null }
    if ($null -ne $av -and $null -ne $bv) { FmtRow "$l p50" $av $bv }
    else { FmtRow "$l p50" ($(if ($null -ne $av) { $av } else { "-" })) ($(if ($null -ne $bv) { $bv } else { "-" })) }
}
Write-Host ""
Write-Host ("样本量：A frames={0}  B frames={1}（过少则加长 -DurationSec）" -f $a.Frames, $b.Frames)
Write-Host "解读：p50/p99 越低越好；A 相对 B 的差值 = 脏区上传+跳帧架构的实际收益。"

# 还原本轮开始前被备份的同分钟会话日志（各组日志已另存 tools\ab_perf_*.log）
if (Test-Path $logDir) {
    Get-ChildItem -Path $logDir -Filter "sysoverlay_*.log.ab_bak" -File -ErrorAction SilentlyContinue |
        ForEach-Object { Move-Item $_.FullName ($_.FullName -replace '\.ab_bak$', '') -Force }
}
