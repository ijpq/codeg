param(
  [Parameter(Mandatory=$true)][int]$RootProcessId,
  [Parameter(Mandatory=$true)][string]$ExpectedExecutable,
  [Parameter(Mandatory=$true)][string]$OwnedStartedAt
)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or -not $env:RUNNER_TEMP) {
  throw 'Abrupt interruption is restricted to an owned isolated CI process'
}
$temporary = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\') + '\'
$expected = [IO.Path]::GetFullPath($ExpectedExecutable)
if (-not $expected.StartsWith($temporary, [StringComparison]::OrdinalIgnoreCase)) {
  throw 'Owned executable must be under RUNNER_TEMP'
}
$rootProcess = Get-Process -Id $RootProcessId -ErrorAction Stop
$started = [DateTime]::Parse($OwnedStartedAt).ToUniversalTime()
if (-not [String]::Equals($rootProcess.Path, $expected, [StringComparison]::OrdinalIgnoreCase) -or
    $rootProcess.StartTime.ToUniversalTime() -lt $started.AddSeconds(-10) -or
    $rootProcess.StartTime.ToUniversalTime() -gt $started.AddSeconds(2)) {
  throw 'Owned root identity changed before interruption'
}
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class CodegIsolatedInterruption {
  [DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr handle);
  [DllImport("ntdll.dll")] public static extern int NtResumeProcess(IntPtr handle);
}
'@
$suspended = $false
try {
  # taskkill /T is children-first. Freeze the app writer first so child EOF
  # cannot turn an intended app crash into a durably completed failed handoff.
  $handle = $rootProcess.Handle
  if ([CodegIsolatedInterruption]::NtSuspendProcess($handle) -ne 0) {
    throw 'Cannot suspend the owned root before abrupt interruption'
  }
  $suspended = $true
  $startInfo = New-Object System.Diagnostics.ProcessStartInfo
  $startInfo.FileName = Join-Path $env:SystemRoot 'System32\taskkill.exe'
  $startInfo.Arguments = "/PID $RootProcessId /T /F"
  $startInfo.UseShellExecute = $false
  $startInfo.CreateNoWindow = $true
  $startInfo.RedirectStandardOutput = $true
  $startInfo.RedirectStandardError = $true
  $killer = New-Object System.Diagnostics.Process
  $killer.StartInfo = $startInfo
  if (-not $killer.Start()) { throw 'Cannot start owned-tree cleanup' }
  $killer.BeginOutputReadLine()
  $killer.BeginErrorReadLine()
  if (-not $killer.WaitForExit(10000)) { throw 'Owned-tree cleanup timed out' }
  $killer.WaitForExit()
  $killCode = $killer.ExitCode
  $killer.Dispose()
  if (-not $rootProcess.WaitForExit(10000)) { throw 'Owned root did not exit' }
  @{rootPid=$RootProcessId;suspendedRoot=$true;rootExited=$true;taskkillExitCode=$killCode} | ConvertTo-Json -Compress
} finally {
  if ($suspended -and -not $rootProcess.HasExited) {
    [void][CodegIsolatedInterruption]::NtResumeProcess($rootProcess.Handle)
  }
  $rootProcess.Dispose()
}
