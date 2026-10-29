param(
  [Parameter(Mandatory=$true)][string]$ReleaseTag,
  [Parameter(Mandatory=$true)][string]$Repository,
  [Parameter(Mandatory=$true)][string]$Destination,
  [Parameter(Mandatory=$true)][string]$SetupManifest,
  [string]$UpgradeFromTag = 'v0.40.0-fix4',
  [switch]$RequirePublished
)
$ErrorActionPreference = 'Stop'
$LegacyBusinessTag = 'v0.40.0-fix6'
$version = $ReleaseTag.TrimStart('v')
$setupName = "codeg_${version}_x64-setup.exe"
$expectedNames = @($setupName, "$setupName.sha256", 'codeg-server-windows-x64.zip', 'codeg-server-windows-x64.zip.sha256') | Sort-Object
# gh release view resolves both published releases and pending draft tags.
$release = gh release view $ReleaseTag --repo $Repository --json tagName,isDraft,isPrerelease,assets | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Cannot read release metadata' }
if ($release.tagName -ne $ReleaseTag -or ($RequirePublished -and $release.isDraft) -or $release.isPrerelease) {
  throw 'Release tag or publication state does not match'
}
if (Compare-Object @($release.assets.name | Sort-Object) $expectedNames) { throw 'Unexpected asset set' }
New-Item -ItemType Directory -Force $Destination | Out-Null
gh release download $ReleaseTag --repo $Repository --dir $Destination
if ($LASTEXITCODE -ne 0) { throw 'Release download failed' }
$evidence = @()
foreach ($name in @($setupName, 'codeg-server-windows-x64.zip')) {
  $file = Join-Path $Destination $name
  $expectedHash = ((Get-Content "$file.sha256" -Raw).Trim() -split '\s+')[0].ToLower()
  if ($expectedHash -notmatch '^[a-f0-9]{64}$') { throw "Invalid checksum for $name" }
  $actualHash = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLower()
  $asset = $release.assets | Where-Object name -eq $name
  if ($actualHash -ne $expectedHash -or (Get-Item $file).Length -ne $asset.size) { throw "Hash or size mismatch for $name" }
  if ($asset.digest -and $asset.digest -ne "sha256:$actualHash") { throw "API digest mismatch for $name" }
  $evidence += @{ name=$name; size=$asset.size; sha256=$actualHash }
}
$expanded = Join-Path $Destination 'expanded'
Expand-Archive (Join-Path $Destination 'codeg-server-windows-x64.zip') $expanded
$root = Join-Path $expanded 'codeg-server-windows-x64'
foreach ($name in @('codeg-server.exe','codeg-mcp.exe','codeg-computer-helper.exe','codeg-cli.exe','web/index.html')) {
  if (-not (Test-Path (Join-Path $root $name) -PathType Leaf)) { throw "Server package missing $name" }
}
python -B scripts/smoke-server-package.py $root $version --require-runtime --require-pi-runtime --log-dir (Join-Path $Destination 'server-smoke')
if ($LASTEXITCODE -ne 0) { throw 'Downloaded Server ZIP startup failed' }
$installed = Join-Path $Destination 'installed'
$process = Start-Process (Join-Path $Destination $setupName) -ArgumentList '/S',"/D=$installed" -Wait -PassThru
if ($process.ExitCode -ne 0) { throw "Downloaded Setup installation failed: $($process.ExitCode)" }
$manifest = Get-Content $SetupManifest -Raw | ConvertFrom-Json
if ($manifest.version -ne $version -or $manifest.commit -ne $env:GITHUB_SHA) { throw 'Setup manifest source mismatch' }
foreach ($file in $manifest.files) {
  $path = Join-Path $installed $file.path
  if (-not (Test-Path $path -PathType Leaf) -or (Get-FileHash -Algorithm SHA256 $path).Hash.ToLower() -ne $file.sha256) { throw "Installed payload mismatch: $($file.path)" }
}
python -B scripts/release-resources.py verify (Join-Path $installed 'web') $version
if ($LASTEXITCODE -ne 0) { throw 'Installed frontend/migration inventory failed' }
python -B scripts/build-pi-runtime.py --output-dir (Join-Path $installed 'pi-runtime') --verify
if ($LASTEXITCODE -ne 0) { throw 'Installed Pi runtime inventory failed' }
python -B scripts/build-codex-adapter.py --output-dir (Join-Path $installed 'runtime') --verify
if ($LASTEXITCODE -ne 0) { throw 'Installed Codex adapter inventory failed' }
$mcpVersion = (& (Join-Path $installed 'codeg-mcp.exe') --version | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $mcpVersion -ne $version) { throw 'Installed MCP version mismatch' }
python -B scripts/verify-windows-dependencies.py $installed --report (Join-Path $Destination 'setup-windows-dependencies.json')
if ($LASTEXITCODE -ne 0) { throw 'Installed Windows runtime dependency verification failed' }
$previous = Join-Path $Destination 'previous-release'
New-Item -ItemType Directory -Force $previous | Out-Null
$oldVersion = $UpgradeFromTag.TrimStart('v')
$oldSetup = "codeg_${oldVersion}_x64-setup.exe"
gh release download $UpgradeFromTag --repo $Repository --dir $previous --pattern $oldSetup --pattern "$oldSetup.sha256" --pattern 'codeg-server-windows-x64.zip' --pattern 'codeg-server-windows-x64.zip.sha256'
if ($LASTEXITCODE -ne 0) { throw 'Could not download upgrade baseline' }
$oldHash = ((Get-Content (Join-Path $previous "$oldSetup.sha256") -Raw).Trim() -split '\s+')[0].ToLower()
if ((Get-FileHash (Join-Path $previous $oldSetup) -Algorithm SHA256).Hash.ToLower() -ne $oldHash) { throw 'Upgrade baseline checksum mismatch' }
$oldServerHash=((Get-Content (Join-Path $previous 'codeg-server-windows-x64.zip.sha256') -Raw).Trim() -split '\s+')[0].ToLower()
if ((Get-FileHash (Join-Path $previous 'codeg-server-windows-x64.zip') -Algorithm SHA256).Hash.ToLower() -ne $oldServerHash) { throw 'Server upgrade baseline checksum mismatch' }
Expand-Archive (Join-Path $previous 'codeg-server-windows-x64.zip') (Join-Path $previous 'server')
$previousServer=Join-Path $previous 'server/codeg-server-windows-x64'
$legacy = Join-Path $Destination 'legacy-business-release'
New-Item -ItemType Directory -Force $legacy | Out-Null
$legacyVersion = $LegacyBusinessTag.TrimStart('v')
$legacySetup = "codeg_${legacyVersion}_x64-setup.exe"
gh release download $LegacyBusinessTag --repo $Repository --dir $legacy --pattern $legacySetup --pattern "$legacySetup.sha256" --pattern 'codeg-server-windows-x64.zip' --pattern 'codeg-server-windows-x64.zip.sha256'
if ($LASTEXITCODE -ne 0) { throw 'Could not download legacy business upgrade baseline' }
foreach ($name in @($legacySetup, 'codeg-server-windows-x64.zip')) {
  $path = Join-Path $legacy $name
  $hash = ((Get-Content "$path.sha256" -Raw).Trim() -split '\s+')[0].ToLower()
  if ($hash -notmatch '^[a-f0-9]{64}$' -or (Get-FileHash $path -Algorithm SHA256).Hash.ToLower() -ne $hash) { throw "Legacy baseline checksum mismatch: $name" }
}
Expand-Archive (Join-Path $legacy 'codeg-server-windows-x64.zip') (Join-Path $legacy 'server')
$legacyServer = Join-Path $legacy 'server/codeg-server-windows-x64'
$nativeTools = Join-Path $Destination 'native-agent-tools'
npm install --prefix $nativeTools --global --include=optional --ignore-scripts --no-audit --no-fund '@agentclientprotocol/codex-acp@2.2.2' '@agentclientprotocol/claude-agent-acp@0.94.0'
if ($LASTEXITCODE -ne 0) { throw 'Isolated ACP installation failed' }
python -B scripts/prepare-office-preview-test.py --output $nativeTools
if ($LASTEXITCODE -ne 0) { throw 'Isolated Office converter checksum verification failed' }
node scripts/test-pi-service-tier.mjs --output (Join-Path $Destination 'pi-service-tier') --pi (Join-Path $root 'pi-runtime/pi.cmd') --adapter (Join-Path $root 'pi-runtime/node_modules/pi-acp/dist/index.js')
if ($LASTEXITCODE -ne 0) { throw 'Packaged Pi service-tier protocol verification failed' }
try {
  node scripts/verify-windows-ui.mjs --setup (Join-Path $Destination $setupName) --previous (Join-Path $previous $oldSetup) --server $root --previous-server $previousServer --legacy-previous (Join-Path $legacy $legacySetup) --legacy-previous-server $legacyServer --output (Join-Path $Destination 'ui') --version $version --native-tools $nativeTools
  if ($LASTEXITCODE -ne 0) { throw 'Setup upgrade/startup or actual Server/Desktop UI verification failed' }
} finally {
  & "$PSScriptRoot/collect-owned-process-events.ps1" -EvidenceDirectory (Join-Path $Destination 'ui')
}
@{ tag=$ReleaseTag; commit=$env:GITHUB_SHA; published=(-not $release.isDraft); assets=$evidence; installation='passed'; server_startup='passed'; setup_startup='passed'; upgrade='passed'; actual_ui='passed'; installed_files=$manifest.files.Count } | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $Destination 'verification.json')
Get-Content (Join-Path $Destination 'verification.json')
