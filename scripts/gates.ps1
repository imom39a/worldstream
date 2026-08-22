$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$WorkspaceDir = Split-Path -Parent $PSScriptRoot
Set-Location $WorkspaceDir

$Python = Get-Command python -ErrorAction SilentlyContinue
$PythonArgs = @()
if (-not $Python) {
    $Python = Get-Command py -ErrorAction SilentlyContinue
    $PythonArgs = @('-3')
}
if (-not $Python) { throw 'Python 3.11+ is required for the compatibility gates.' }

$VersionCheck = & $Python.Source @PythonArgs -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 11) else 1)'
if ($LASTEXITCODE -ne 0) { throw 'Python 3.11+ is required for the compatibility gates.' }

& $Python.Source @PythonArgs (Join-Path $PSScriptRoot 'gates.py') @args
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
