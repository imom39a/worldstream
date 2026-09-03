$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$WorkspaceDir = Split-Path -Parent $PSScriptRoot
$ReportPath = $null
$ReleaseInventory = $null
$VerifyArgs = [System.Collections.Generic.List[string]]::new()
for ($Index = 0; $Index -lt $args.Count; $Index++) {
    if ($args[$Index] -eq '--report') {
        if ($Index + 1 -ge $args.Count) { throw 'missing value for --report' }
        $ReportPath = $args[++$Index]
    } elseif ($args[$Index] -like '--report=*') {
        $ReportPath = $args[$Index].Substring(9)
    } elseif ($args[$Index] -eq '--release-inventory') {
        if ($Index + 1 -ge $args.Count) { throw 'missing value for --release-inventory' }
        $ReleaseInventory = $args[++$Index]
    } elseif ($args[$Index] -like '--release-inventory=*') {
        $ReleaseInventory = $args[$Index].Substring(20)
    } else { $VerifyArgs.Add($args[$Index]) }
}
$Artifact = $VerifyArgs | Where-Object { -not $_.StartsWith('-') } | Select-Object -First 1
$Python = $env:WORLDSTREAM_RELEASE_PYTHON
if ([string]::IsNullOrWhiteSpace($Python) -or -not (Test-Path -LiteralPath $Python -PathType Leaf)) {
    throw 'release verification requires an executable WORLDSTREAM_RELEASE_PYTHON'
}
$ExpectedPython = (Get-Content "$WorkspaceDir/.python-version" -Raw).Trim()
$ActualPython = (& $Python -I -c 'import platform; print(platform.python_version())').Trim()
if ($LASTEXITCODE -ne 0 -or $ActualPython -ne $ExpectedPython) {
    throw "release verification requires Python $ExpectedPython, observed $ActualPython"
}
function Invoke-PackagePython {
    param([string[]]$PackageArgs)
    & $Python -I "$WorkspaceDir/scripts/package.py" @PackageArgs
    $script:PackageExitCode = $LASTEXITCODE
}

$IsOciContext = (
    $null -ne $ReportPath -and
    $null -ne $Artifact -and
    (Test-Path -LiteralPath $Artifact -PathType Container) -and
    (Test-Path -LiteralPath (Join-Path $Artifact 'Dockerfile') -PathType Leaf) -and
    (Test-Path -LiteralPath (Join-Path $Artifact 'oci-metadata.json') -PathType Leaf)
)
if ($IsOciContext) {
    $ContextArgs = @('verify') + [string[]]$VerifyArgs
    if ($null -ne $ReleaseInventory) {
        $ContextArgs += @('--release-inventory', $ReleaseInventory)
    }
    $ContextArgs += @('--report', $ReportPath)
    Invoke-PackagePython -PackageArgs $ContextArgs
    if ($PackageExitCode -ne 0) {
        throw "OCI context verification failed with exit code $PackageExitCode"
    }
    exit 0
}

$VerificationArgs = @('verify') + [string[]]$VerifyArgs
if ($null -ne $ReleaseInventory) {
    $VerificationArgs += @('--release-inventory', $ReleaseInventory)
}
Invoke-PackagePython -PackageArgs $VerificationArgs
if ($PackageExitCode -eq 11 -and $VerifyArgs.Contains('--structural-only')) { exit 11 }
if ($PackageExitCode -ne 0) {
    throw "release verification failed with exit code $PackageExitCode"
}
if ($null -eq $ReportPath) { exit 0 }
if ($null -eq $Artifact -or (Test-Path -LiteralPath $Artifact -PathType Container)) {
    throw '--report requires an archive artifact path'
}
$ReportArgs = @('report', $Artifact, '--check', $ReportPath)
if ($null -ne $ReleaseInventory) {
    $ReportArgs += @('--release-inventory', $ReleaseInventory)
}
Invoke-PackagePython -PackageArgs $ReportArgs
if ($PackageExitCode -ne 0) {
    throw "package report verification failed with exit code $PackageExitCode"
}
Write-Output "verified package report ${ReportPath}: $(Split-Path $Artifact -Leaf)"
