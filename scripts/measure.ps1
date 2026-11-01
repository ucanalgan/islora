# Measures CPU and memory against the budget in docs/PLAN.md, including what the
# island costs the system compositor (dwm.exe), which a looping animation can
# hide from the app's own numbers. Run after `cargo build --release`.
# Exits 1 if the budget is exceeded. Real media players are never touched.
param(
    [string]$Exe = "$PSScriptRoot\..\target\release\islora.exe",
    [int]$Seconds = 15,
    # dwm.exe is noisy, since it also draws everything else on screen, so the
    # wave modes are alternated over several rounds and compared by median.
    [int]$Rounds = 5
)

. "$PSScriptRoot\media-fixture.ps1"

$BudgetPrivateMB = 50
$BudgetDemoCpuPercent = 0.5

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Win {
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string n);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
}
'@

function Close-Island {
    $hwnd = [Win]::FindWindowW('IsloraWindow', [NullString]::Value)
    if ($hwnd -ne [IntPtr]::Zero) { [void][Win]::PostMessageW($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) }
    Start-Sleep -Milliseconds 500
    Get-Process islora -ErrorAction SilentlyContinue | Stop-Process -Force
}

# Reading dwm.exe's process times is denied, so this uses the performance
# counter, which is open to everyone. Unit: 100 ns.
function Get-DwmCpuTime {
    (Get-CimInstance Win32_PerfRawData_PerfProc_Process -Filter "Name='dwm'").PercentProcessorTime
}

# Every time a thread of the process was scheduled in, which is what actually
# wakes a CPU. A periodic timer shows up here even when too brief to show as CPU.
function Get-Wakes($ProcessId) {
    $total = [uint64]0
    Get-CimInstance Win32_PerfRawData_PerfProc_Thread -Filter "IDProcess=$ProcessId" |
        ForEach-Object { $total += [uint64]$_.ContextSwitchesPersec }
    $total
}

# Total time dwm.exe spent on the GPU, in 100 ns units.
function Get-DwmGpuTime($DwmId) {
    $total = [uint64]0
    Get-CimInstance Win32_PerfRawData_GPUPerformanceCounters_GPUEngine |
        Where-Object { $_.Name -like "pid_${DwmId}_*" } |
        ForEach-Object { $total += [uint64]$_.RunningTime }
    $total
}

function Measure-Window($Process) {
    $dwm = Get-Process dwm | Select-Object -First 1
    $dwmBefore = Get-DwmCpuTime
    $gpuBefore = Get-DwmGpuTime $dwm.Id
    if ($Process) {
        $Process.Refresh(); $before = $Process.TotalProcessorTime
        $wakesBefore = Get-Wakes $Process.Id
    }

    Start-Sleep -Seconds $Seconds

    $sample = [ordered]@{
        DwmCpuMs = [math]::Round(((Get-DwmCpuTime) - $dwmBefore) / 10000)
        DwmGpuMs = [math]::Round(((Get-DwmGpuTime $dwm.Id) - $gpuBefore) / 10000)
    }
    if ($Process) {
        if ($Process.HasExited) { throw "the app exited during measurement with code $($Process.ExitCode)" }
        $Process.Refresh()
        $sample.CpuMs = ($Process.TotalProcessorTime - $before).TotalMilliseconds
        $sample.Wakes = [math]::Max(0, [double](Get-Wakes $Process.Id) - [double]$wakesBefore)
        $sample.PrivateMB = [math]::Round($Process.PrivateMemorySize64 / 1MB, 1)
        $sample.WorkingMB = [math]::Round($Process.WorkingSet64 / 1MB, 1)
        $sample.Threads = $Process.Threads.Count
    }
    $sample
}

function Measure-Island([string]$Name, [string[]]$Arguments, [switch]$MustShow) {
    $process = Start-Process -FilePath $Exe -ArgumentList $Arguments -PassThru
    Start-Sleep -Seconds 2
    if ($process.HasExited) { throw "$Name`: the app exited during startup with code $($process.ExitCode)" }
    $sample = Measure-Window $process
    # A measurement of a pill that was not there would look like a great result.
    if ($MustShow -and -not [Win]::IsWindowVisible([Win]::FindWindowW('IsloraWindow', [NullString]::Value))) {
        Close-Island
        throw "$Name`: the pill was not on screen, so this sample means nothing"
    }
    Close-Island
    $sample.Scenario = $Name
    [pscustomobject]$sample
}

Get-Process islora -ErrorAction SilentlyContinue | Stop-Process -Force
# A killed copy holds the single-instance lock for a moment.
Start-Sleep -Milliseconds 800

$fixture = New-MediaFixture (Join-Path $env:TEMP 'islora-measure')
$playerLog = Join-Path $env:TEMP 'islora-measure\player.txt'
# Generous on purpose: the counters above take seconds to read, and a player that
# quits early would silently turn the last samples into measurements of nothing.
$playerSeconds = $Rounds * 2 * ($Seconds + 20) + 60

$baseline = Measure-Window $null
$baseline.Scenario = 'no app (baseline)'
$baseline = [pscustomobject]$baseline

$idle = Measure-Island 'idle (hidden)' $NoSession
$demo = Measure-Island 'demo (all transitions)' ($NoSession + '--demo')

$player = Start-TestPlayer $fixture $playerSeconds $playerLog
Start-Sleep -Seconds 1
$samples = @{ always = @(); still = @() }
foreach ($round in 1..$Rounds) {
    foreach ($motion in 'always', 'still') {
        $samples[$motion] += Measure-Island "playing, $motion wave" @('--only-session', $TestSession, '--wave', $motion) -MustShow
    }
}
Stop-Process -Id $player.Id -Force -ErrorAction SilentlyContinue

function Get-Median([double[]]$Values) {
    $sorted = @($Values | Sort-Object)
    $sorted[[int][math]::Floor($sorted.Count / 2)]
}

$waves = foreach ($motion in 'always', 'still') {
    $rows = $samples[$motion]
    [pscustomobject]@{
        Scenario  = "playing, $motion wave (median of $Rounds)"
        CpuMs     = Get-Median $rows.CpuMs
        Wakes     = Get-Median $rows.Wakes
        PrivateMB = ($rows.PrivateMB | Measure-Object -Maximum).Maximum
        WorkingMB = ($rows.WorkingMB | Measure-Object -Maximum).Maximum
        Threads   = ($rows.Threads | Measure-Object -Maximum).Maximum
        DwmCpuMs  = Get-Median $rows.DwmCpuMs
        DwmGpuMs  = Get-Median $rows.DwmGpuMs
    }
}

$rows = @($baseline, $idle, $demo) + $waves
$rows | Select-Object Scenario, CpuMs, Wakes, PrivateMB, WorkingMB, Threads, DwmCpuMs, DwmGpuMs | Format-Table -AutoSize | Out-String
foreach ($motion in 'always', 'still') {
    "dwm GPU ms per round, ${motion}: $($samples[$motion].DwmGpuMs -join ', ')"
}

$failures = @()
# Windows counts CPU time in ticks of 15.625 ms, so one tick is a single event.
if ($idle.CpuMs -gt 15.625) { $failures += "idle CPU is not zero: $($idle.CpuMs) ms" }
foreach ($row in @($idle, $demo) + $waves) {
    if ($row.PrivateMB -gt $BudgetPrivateMB) { $failures += "$($row.Scenario) private memory over budget: $($row.PrivateMB) MB" }
}
$demoPercent = $demo.CpuMs / ($Seconds * 1000 * [Environment]::ProcessorCount) * 100
if ($demoPercent -gt $BudgetDemoCpuPercent) { $failures += "demo CPU over budget: $demoPercent%" }

if ($failures) {
    $failures | ForEach-Object { "FAIL: $_" }
    exit 1
}
'PASS: within budget'
