$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$WorkspaceDir = Split-Path -Parent $PSScriptRoot
git -C $WorkspaceDir config core.hooksPath .githooks
if ($LASTEXITCODE -ne 0) { throw "git config failed with exit code $LASTEXITCODE" }
Write-Host 'Configured core.hooksPath=.githooks for this checkout.'
