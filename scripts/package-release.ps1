$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$WorkspaceDir = Split-Path -Parent $PSScriptRoot
$ReportPath = $null
$PackageArgs = [System.Collections.Generic.List[string]]::new()
for ($Index = 0; $Index -lt $args.Count; $Index++) {
    if ($args[$Index] -eq '--report') {
        if ($Index + 1 -ge $args.Count) { throw 'missing value for --report' }
        $ReportPath = $args[++$Index]
    } elseif ($args[$Index] -like '--report=*') {
        $ReportPath = $args[$Index].Substring(9)
    } else {
        $PackageArgs.Add($args[$Index])
    }
}
$Python = Get-Command py -ErrorAction SilentlyContinue
if ($null -ne $Python) {
    & $Python.Source -3 "$WorkspaceDir/scripts/package.py" package @PackageArgs
} else {
    & python "$WorkspaceDir/scripts/package.py" package @PackageArgs
}
if ($LASTEXITCODE -ne 0) { throw "release packaging failed with exit code $LASTEXITCODE" }
if ($PackageArgs -contains '--dry-run') { exit 0 }
$Target = $null
$Output = Join-Path $WorkspaceDir 'dist'
for ($Index = 0; $Index -lt $PackageArgs.Count; $Index++) {
    if ($PackageArgs[$Index] -eq '--target') { $Target = $PackageArgs[++$Index] }
    elseif ($PackageArgs[$Index] -like '--target=*') { $Target = $PackageArgs[$Index].Substring(9) }
    elseif ($PackageArgs[$Index] -eq '--output') { $Output = $PackageArgs[++$Index] }
    elseif ($PackageArgs[$Index] -like '--output=*') { $Output = $PackageArgs[$Index].Substring(9) }
}
$Version = ([regex]::Match((Get-Content "$WorkspaceDir/compatibility.toml" -Raw), '(?m)^product\s*=\s*"([^"]+)"')).Groups[1].Value
if (-not $Version) { throw 'compatibility.toml has no product version' }
$Suffix = switch ($Target) {
    'source' { '.tar.gz' }
    'linux-x86_64' { '.tar.gz' }
    'windows-x64' { '.zip' }
    default { throw "unsupported or missing package target: $Target" }
}
$Artifact = Join-Path $Output "worldstream-$Version-$Target$Suffix"
if (-not (Test-Path -LiteralPath $Artifact -PathType Leaf)) { throw "packaging did not produce expected archive: $Artifact" }
$ReportArgs = [System.Collections.Generic.List[string]]::new()
$ReportArgs.Add('report')
$ReportArgs.Add($Artifact)
if ($null -ne $ReportPath) {
    $ReportArgs.Add('--report')
    $ReportArgs.Add($ReportPath)
}
if ($null -ne $Python) {
    & $Python.Source -3 "$WorkspaceDir/scripts/package.py" @ReportArgs
} else {
    & python "$WorkspaceDir/scripts/package.py" @ReportArgs
}
if ($LASTEXITCODE -ne 0) { throw "release archive report/verification failed with exit code $LASTEXITCODE" }
