param([Parameter(Mandatory=$true)][string]$EvidenceDirectory)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'Only collect isolated CI process events' }
$root = [IO.Path]::GetFullPath($EvidenceDirectory)
$temporary = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\') + '\'
if (-not $root.StartsWith($temporary, [StringComparison]::OrdinalIgnoreCase)) { throw 'Evidence must be under RUNNER_TEMP' }
$lifecycle = Join-Path $root 'owned-process-lifecycle.log'
if (-not (Test-Path $lifecycle)) { return }
$records = @(Get-Content $lifecycle | ForEach-Object { $_ | ConvertFrom-Json })
$owned = @($records | Where-Object event -eq 'spawn')
if (-not $owned.Count) { return }
$started = [DateTime]::Parse(($owned | Sort-Object startedAt | Select-Object -First 1).startedAt).ToUniversalTime()
$processIds = @($owned | ForEach-Object { [long]$_.pid })
# Event messages and dumps may contain environment/arguments. Export only OS
# metadata from the faulting owned executable; never dump process memory.
$allowed = @('AppName','AppVersion','AppTimeStamp','ModuleName','ModuleVersion','ModuleTimeStamp','ExceptionCode','FaultingOffset','ProcessId','ProcessCreationTime','AppPath','ModulePath','IntegratorReportId')
$evidence = @()
try {
  $events = @(Get-WinEvent -FilterHashtable @{LogName='Application';Id=@(1000,1001,1002);StartTime=$started} -ErrorAction SilentlyContinue)
  foreach ($event in $events) {
    [xml]$xml = $event.ToXml()
    $fields = @{}
    foreach ($field in $xml.Event.EventData.Data) {
      if ($allowed -contains $field.Name) { $fields[$field.Name] = $field.InnerText }
    }
    $processId = 0L
    if ($fields.ProcessId -match '^0x') { $processId = [Convert]::ToInt64($fields.ProcessId.Substring(2), 16) }
    elseif ($fields.ProcessId -match '^\d+$') { $processId = [long]$fields.ProcessId }
    if ($processIds -contains $processId -and $fields.AppName -match '^codeg(-server)?\.exe$') {
      $evidence += @{time=$event.TimeCreated.ToUniversalTime().ToString('o');id=$event.Id;provider=$event.ProviderName;fields=$fields}
    }
  }
  @{collected=$true;started=$started.ToString('o');owned_pids=$processIds;events=$evidence} | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $root 'owned-process-windows-events.json')
} catch {
  @{collected=$false;error_type=$_.Exception.GetType().FullName} | ConvertTo-Json | Set-Content (Join-Path $root 'owned-process-windows-events.json')
}
