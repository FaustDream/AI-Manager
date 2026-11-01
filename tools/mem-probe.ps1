# AI Manager 内存采样（架构 §9.2 / §9.4 验收用）。
#
# 用法（在本目录执行）：
#   powershell -ExecutionPolicy Bypass -File mem-probe.ps1                      # 默认 5 分钟
#   powershell -ExecutionPolicy Bypass -File mem-probe.ps1 -Duration 86400      # 24h 挂机
#   powershell -ExecutionPolicy Bypass -File mem-probe.ps1 -Duration 604800     # 7 天
#   powershell -ExecutionPolicy Bypass -File mem-probe.ps1 -Interval 60 -Duration 300
#
# 采样对象：主进程（ai-manager.exe）+ 它名下的全部 WebView2 子进程（私有提交 + 工作集 + 线程）。
# 输出：CSV 一行一次采样（Excel 可直接看），结束时打印 min/avg/max 摘要与退出事件。
# 「反复切页 50 次」验收：先跑本脚本，然后手动切页 50 次，看曲线是否回同一水平。

param(
    [string]$Name = "ai-manager",
    [int]$Duration = 300,
    [int]$Interval = 10,
    [string]$CsvPath = ""
)

$ErrorActionPreference = "Stop"
if (-not $CsvPath) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $CsvPath = Join-Path $PWD "mem-probe-$stamp.csv"
}

function Get-Tree {
    # 返回 @{ Main = 进程对象或 $null; Children = WebView2 子进程数组 }
    $main = Get-Process -Name $Name -ErrorAction SilentlyContinue |
        Where-Object { $_.MainWindowTitle -ne $null } | Select-Object -First 1
    if (-not $main) {
        $main = Get-Process -Name $Name -ErrorAction SilentlyContinue | Select-Object -First 1
    }
    if (-not $main) { return $null }
    $children = @()
    try {
        $kids = Get-CimInstance Win32_Process -Filter "ParentProcessId = $($main.Id)" -ErrorAction SilentlyContinue
        foreach ($k in $kids) {
            if ($k.Name -like "msedgewebview2*") {
                $p = Get-Process -Id $k.ProcessId -ErrorAction SilentlyContinue
                if ($p) { $children += $p }
            }
        }
    } catch {}
    return @{ Main = $main; Children = $children }
}

"sample,time,main_private_mb,main_ws_mb,main_threads,webview_private_mb,webview_ws_mb,webview_procs,total_private_mb,note" |
    Out-File -FilePath $CsvPath -Encoding utf8

Write-Host "采样 $Duration 秒 / 间隔 $Interval 秒 → $CsvPath"
Write-Host "先启动 AI Manager 再开始，或让本脚本先跑起来（启动前基线也会被记录）。"

$start = Get-Date
$n = 0
$exitEvent = $false
while (((Get-Date) - $start).TotalSeconds -lt $Duration) {
    $tree = Get-Tree
    if (-not $tree) {
        if ($n -gt 0) {
            Write-Host "主进程消失了——如果没人手动关它，这就是一次崩溃，检查 data\logs\app.log"
            Add-Content -Path $CsvPath -Value "$n,$(Get-Date -Format o),,,,,,,app-exited"
            $exitEvent = $true
            break
        }
        Write-Host "未找到 $Name 进程，等待中…"
    } else {
        $m = $tree.Main
        $cp = 0.0; $cw = 0.0; $cn = 0
        foreach ($c in $tree.Children) {
            $cp += $c.PrivateMemorySize64 / 1MB
            $cw += $c.WorkingSet64 / 1MB
            $cn += 1
        }
        $mp = $m.PrivateMemorySize64 / 1MB
        $mw = $m.WorkingSet64 / 1MB
        $total = $mp + $cp
        $note = if ($env:MEM_NOTE) { $env:MEM_NOTE } else { "" }
        "$n,$(Get-Date -Format o),$([math]::Round($mp,1)),$([math]::Round($mw,1)),$($m.Threads.Count),$([math]::Round($cp,1)),$([math]::Round($cw,1)),$cn,$([math]::Round($total,1)),$note" |
            Add-Content -Path $CsvPath
        Write-Host ("#{0} 主 {1} MB | WebView2×{2} {3} MB | 合计私有 {4} MB" -f `
            $n, [math]::Round($mp,1), $cn, [math]::Round($cp,1), [math]::Round($total,1))
    }
    $n++
    Start-Sleep -Seconds $Interval
}

if (-not $exitEvent) {
    $rows = Import-Csv $CsvPath | Where-Object { $_.total_private_mb }
    if ($rows) {
        $total = $rows | ForEach-Object { [double]$_.total_private_mb }
        $main = $rows | ForEach-Object { [double]$_.main_private_mb }
        Write-Host "`n===== 摘要（私有提交 MB）====="
        Write-Host ("合计  min/avg/max：{0} / {1} / {2}" -f `
            [math]::Round(($total | Measure-Object -Minimum).Minimum,1),
            [math]::Round(($total | Measure-Object -Average).Average,1),
            [math]::Round(($total | Measure-Object -Maximum).Maximum,1))
        Write-Host ("主进程 min/avg/max：{0} / {1} / {2}" -f `
            [math]::Round(($main | Measure-Object -Minimum).Minimum,1),
            [math]::Round(($main | Measure-Object -Average).Average,1),
            [math]::Round(($main | Measure-Object -Maximum).Maximum,1))
        Write-Host "阈值：空闲合计 < 250MB（主 < 80MB）；24h 波动 < 10% 且不单调上升"
    }
}
Write-Host "CSV：$CsvPath"
