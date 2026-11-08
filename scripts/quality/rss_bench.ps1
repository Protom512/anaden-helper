param(
  [Parameter(Mandatory = $true)][string]$Command,
  [string[]]$Arguments = @(),
  [string]$WorkingDirectory = (Get-Location).Path,
  [int]$Repeat = 5,
  [int]$TimeoutSec = 300,
  [string]$Label = 'workload',
  [string]$JsonOut
)

$runs = New-Object System.Collections.Generic.List[object]
$anyFailed = $false

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class PeakRssInterop {
  [StructLayout(LayoutKind.Sequential)]
  public struct PROCESS_MEMORY_COUNTERS_EX {
    public uint cb;
    public uint PageFaultCount;
    public UIntPtr PeakWorkingSetSize;
    public UIntPtr WorkingSetSize;
    public UIntPtr QuotaPeakPagedPoolUsage;
    public UIntPtr QuotaPagedPoolUsage;
    public UIntPtr QuotaPeakNonPagedPoolUsage;
    public UIntPtr QuotaNonPagedPoolUsage;
    public UIntPtr PagefileUsage;
    public UIntPtr PeakPagefileUsage;
    public UIntPtr PrivateUsage;
  }
  [DllImport("psapi.dll", SetLastError = true)]
  private static extern bool GetProcessMemoryInfo(IntPtr hProcess, out PROCESS_MEMORY_COUNTERS_EX counters, uint cb);
  public static long GetPeakWorkingSet(IntPtr handle) {
    PROCESS_MEMORY_COUNTERS_EX counters = new PROCESS_MEMORY_COUNTERS_EX();
    counters.cb = (uint)Marshal.SizeOf(typeof(PROCESS_MEMORY_COUNTERS_EX));
    if (!GetProcessMemoryInfo(handle, out counters, counters.cb)) {
      throw new InvalidOperationException("GetProcessMemoryInfo failed: " + Marshal.GetLastWin32Error());
    }
    return (long)counters.PeakWorkingSetSize;
  }
}
"@

function ConvertTo-ArgumentLiteral([string]$Value) {
  if ($Value -eq '') { return '""' }
  if ($Value -notmatch '[\s"]') { return $Value }
  $escaped = $Value -replace '(\\*)"', '$1$1\"'
  $escaped = $escaped -replace '(\\+)$', '$1$1'
  return '"' + $escaped + '"'
}

for ($i = 0; $i -lt $Repeat; $i++) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $Command
  $psi.Arguments = (@($Arguments) | ForEach-Object { ConvertTo-ArgumentLiteral $_ }) -join ' '
  $psi.WorkingDirectory = $WorkingDirectory
  $psi.UseShellExecute = $false
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.CreateNoWindow = $true
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  try {
    $proc = [System.Diagnostics.Process]::Start($psi)
  } catch {
    Write-Error "spawn failed: $Command ($($_.Exception.Message))"
    exit 2
  }
  $outTask = $proc.StandardOutput.ReadToEndAsync()
  $errTask = $proc.StandardError.ReadToEndAsync()
  if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
    Write-Error "timeout after ${TimeoutSec}s: $Command $($Arguments -join ' ')"
    try { $proc.Kill() } catch { }
    exit 2
  }
  $sw.Stop()
  $exitCode = $proc.ExitCode
  try {
    $peakRss = [PeakRssInterop]::GetPeakWorkingSet($proc.Handle)
  } catch {
    Write-Error "peak RSS query failed: $($_.Exception.Message)"
    try { $proc.Dispose() } catch { }
    exit 2
  }
  if ($exitCode -ne 0) { $anyFailed = $true }
  $runs.Add([ordered]@{
    run       = $i + 1
    exitCode  = $exitCode
    peakRssBytes = $peakRss
    durationMs   = $sw.ElapsedMilliseconds
  })
  $proc.Dispose()
}

function Get-Median([long[]]$values) {
  if ($values.Count -eq 0) {
    throw "Get-Median: empty array (-validate -Repeat >= 1)"
  }
  $sorted = $values | Sort-Object
  $mid = [int][Math]::Floor($sorted.Count / 2)
  if ($sorted.Count % 2 -eq 1) {
    return [long]$sorted[$mid]
  }
  return [long](($sorted[$mid - 1] + $sorted[$mid]) / 2)
}

$peakValues = @($runs | ForEach-Object { [long]$_.peakRssBytes })
$durValues = @($runs | ForEach-Object { [long]$_.durationMs })
$report = [ordered]@{
  generatedAt = (Get-Date).ToUniversalTime().ToString('o')
  label       = $Label
  command     = $Command
  arguments   = $Arguments
  workingDirectory = $WorkingDirectory
  repeat      = $Repeat
  runs        = $runs
  summary     = [ordered]@{
    peakRssBytesMedian = Get-Median $peakValues
    peakRssBytesMin    = ($peakValues | Measure-Object -Minimum).Minimum
    peakRssBytesMax    = ($peakValues | Measure-Object -Maximum).Maximum
    durationMsMedian   = Get-Median $durValues
  }
}

$json = $report | ConvertTo-Json -Depth 6
if ($JsonOut) {
  $jsonOutFull = [System.IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($JsonOut))
  $jsonDir = Split-Path -Parent $jsonOutFull
  if (-not (Test-Path $jsonDir)) { New-Item -ItemType Directory -Path $jsonDir -Force | Out-Null }
  [System.IO.File]::WriteAllText($jsonOutFull, $json)
  Write-Output "rss_bench: wrote $jsonOutFull"
} else {
  Write-Output $json
}

if ($anyFailed) {
  Write-Error "rss_bench: at least one run exited non-zero — workload is not clean"
  exit 1
}
Write-Output ("rss_bench: {0}: median peak RSS {1:N0} bytes, median wall {2} ms ({3} runs)" -f `
  $Label, $report.summary.peakRssBytesMedian, $report.summary.durationMsMedian, $Repeat)
exit 0
