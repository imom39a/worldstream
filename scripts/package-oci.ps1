$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$WorkspaceDir = Split-Path -Parent $PSScriptRoot
$Python = $env:WORLDSTREAM_RELEASE_PYTHON
if ([string]::IsNullOrWhiteSpace($Python) -or -not (Test-Path -LiteralPath $Python -PathType Leaf)) {
    throw 'OCI packaging requires an executable WORLDSTREAM_RELEASE_PYTHON'
}
$ExpectedPython = (Get-Content "$WorkspaceDir/.python-version" -Raw).Trim()
$ActualPython = (& $Python -I -c 'import platform; print(platform.python_version())').Trim()
if ($LASTEXITCODE -ne 0 -or $ActualPython -ne $ExpectedPython) {
    throw "OCI packaging requires Python $ExpectedPython, observed $ActualPython"
}
& $Python -I "$WorkspaceDir/scripts/package.py" oci-context @args
if ($LASTEXITCODE -ne 0) { throw "OCI context generation failed with exit code $LASTEXITCODE" }
