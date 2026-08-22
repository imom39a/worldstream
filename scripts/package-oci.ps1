$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$WorkspaceDir = Split-Path -Parent $PSScriptRoot
$Python = Get-Command py -ErrorAction SilentlyContinue
if ($null -ne $Python) {
    & $Python.Source -3 "$WorkspaceDir/scripts/package.py" oci-context @args
} else {
    & python "$WorkspaceDir/scripts/package.py" oci-context @args
}
if ($LASTEXITCODE -ne 0) { throw "OCI context generation failed with exit code $LASTEXITCODE" }
