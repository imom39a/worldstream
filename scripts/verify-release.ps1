$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$WorkspaceDir = Split-Path -Parent $PSScriptRoot
$ReportPath = $null
$VerifyArgs = [System.Collections.Generic.List[string]]::new()
for ($Index = 0; $Index -lt $args.Count; $Index++) {
    if ($args[$Index] -eq '--report') {
        if ($Index + 1 -ge $args.Count) { throw 'missing value for --report' }
        $ReportPath = $args[++$Index]
    } elseif ($args[$Index] -like '--report=*') {
        $ReportPath = $args[$Index].Substring(9)
    } else { $VerifyArgs.Add($args[$Index]) }
}
$Artifact = $VerifyArgs | Where-Object { -not $_.StartsWith('-') } | Select-Object -First 1
$Python = Get-Command py -ErrorAction SilentlyContinue
function Invoke-PackagePython {
    param([string[]]$PackageArgs)
    if ($null -ne $Python) {
        & $Python.Source -3 "$WorkspaceDir/scripts/package.py" @PackageArgs
    } else {
        & python "$WorkspaceDir/scripts/package.py" @PackageArgs
    }
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
    $ContextArgs = @('verify') + [string[]]$VerifyArgs + @('--report', $ReportPath)
    Invoke-PackagePython -PackageArgs $ContextArgs
    if ($PackageExitCode -ne 0) {
        throw "OCI context verification failed with exit code $PackageExitCode"
    }
    exit 0
}

$VerificationArgs = @('verify') + [string[]]$VerifyArgs
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
Invoke-PackagePython -PackageArgs $ReportArgs
if ($PackageExitCode -ne 0) {
    throw "package report verification failed with exit code $PackageExitCode"
}
Write-Output "verified package report ${ReportPath}: $(Split-Path $Artifact -Leaf)"
