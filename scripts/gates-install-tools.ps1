$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Byte-pinned hosted Windows scanner installation. The local hook never calls
# this script and therefore never mutates a developer toolchain.
$GitleaksVersion = '8.29.1'
$GitleaksSha256 = 'e4b7d556f0cddbe23d10d8fac2ab0f29f68f019091c6599ffbeaa8a4fb71ac78'
$CargoAuditVersion = '0.22.2'
$CargoAuditSha256 = '0a7316540862c13d954f648917ceacca593747baed6eec180fafa590be2710ab'

if (-not $env:RUNNER_TEMP -or -not $env:GITHUB_PATH) {
    throw 'RUNNER_TEMP and GITHUB_PATH are required'
}
$InstallDir = Join-Path $env:RUNNER_TEMP 'worldstream-gate-tools'
$TemporaryDir = Join-Path $env:RUNNER_TEMP ("worldstream-gate-tools-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $InstallDir, $TemporaryDir | Out-Null
try {
    $GitleaksArchive = "gitleaks_${GitleaksVersion}_windows_x64.zip"
    $CargoAuditArchive = "cargo-audit-x86_64-pc-windows-msvc-v${CargoAuditVersion}.zip"
    $GitleaksPath = Join-Path $TemporaryDir $GitleaksArchive
    $CargoAuditPath = Join-Path $TemporaryDir $CargoAuditArchive
    Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/gitleaks/gitleaks/releases/download/v${GitleaksVersion}/${GitleaksArchive}" -OutFile $GitleaksPath
    Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/rustsec/rustsec/releases/download/cargo-audit/v${CargoAuditVersion}/${CargoAuditArchive}" -OutFile $CargoAuditPath
    if ((Get-FileHash -Algorithm SHA256 $GitleaksPath).Hash.ToLowerInvariant() -ne $GitleaksSha256) {
        throw 'gitleaks release asset digest mismatch'
    }
    if ((Get-FileHash -Algorithm SHA256 $CargoAuditPath).Hash.ToLowerInvariant() -ne $CargoAuditSha256) {
        throw 'cargo-audit release asset digest mismatch'
    }
    $GitleaksExtract = Join-Path $TemporaryDir 'gitleaks'
    $CargoAuditExtract = Join-Path $TemporaryDir 'cargo-audit'
    Expand-Archive -LiteralPath $GitleaksPath -DestinationPath $GitleaksExtract
    Expand-Archive -LiteralPath $CargoAuditPath -DestinationPath $CargoAuditExtract
    Copy-Item -LiteralPath (Join-Path $GitleaksExtract 'gitleaks.exe') -Destination (Join-Path $InstallDir 'gitleaks.exe')
    $CargoAuditBinaries = @(Get-ChildItem -LiteralPath $CargoAuditExtract -Filter 'cargo-audit.exe' -Recurse)
    if ($CargoAuditBinaries.Count -ne 1) {
        throw 'cargo-audit archive must contain exactly one binary'
    }
    Copy-Item -LiteralPath $CargoAuditBinaries[0].FullName -Destination (Join-Path $InstallDir 'cargo-audit.exe')
    if (-not ((& (Join-Path $InstallDir 'gitleaks.exe') version) -match [regex]::Escape($GitleaksVersion))) {
        throw 'unexpected gitleaks version'
    }
    if (-not ((& (Join-Path $InstallDir 'cargo-audit.exe') --version) -match "cargo-audit $([regex]::Escape($CargoAuditVersion))")) {
        throw 'unexpected cargo-audit version'
    }
    # Prime the RustSec advisory database during the explicitly online setup
    # phase. The actual gate audits Cargo.lock with --no-fetch.
    'version = 3' | & (Join-Path $InstallDir 'cargo-audit.exe') audit --file -
    if ($LASTEXITCODE -ne 0) {
        throw 'cargo-audit advisory database prime failed'
    }
    $InstallDir | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append
}
finally {
    Remove-Item -LiteralPath $TemporaryDir -Recurse -Force -ErrorAction SilentlyContinue
}
