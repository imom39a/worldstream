$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($PSVersionTable.PSVersion -lt [version]'7.4') {
    throw 'PowerShell 7.4 or newer is required for native command failure handling.'
}
$PSNativeCommandUseErrorActionPreference = $true

$WorkspaceDir = Split-Path -Parent $PSScriptRoot
Set-Location $WorkspaceDir

$SmokeRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("worldstream-smoke-" + [guid]::NewGuid())
$DataDir = Join-Path $SmokeRoot 'data'
$ServerOut = Join-Path $SmokeRoot 'worldstreamd.out.log'
$ServerErr = Join-Path $SmokeRoot 'worldstreamd.err.log'
$SmokePort = if ($env:WORLDSTREAM_SMOKE_PORT) {
    $env:WORLDSTREAM_SMOKE_PORT
} else {
    $PortProbe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $PortProbe.Start()
    try {
        ([System.Net.IPEndPoint]$PortProbe.LocalEndpoint).Port.ToString()
    } finally {
        $PortProbe.Stop()
    }
}
$BindAddress = "127.0.0.1:$SmokePort"
$BaseUrl = "http://$BindAddress"
$Daemon = Join-Path $WorkspaceDir 'target/debug/worldstreamd.exe'
$Control = Join-Path $WorkspaceDir 'target/debug/worldstreamctl.exe'

[void](New-Item -ItemType Directory -Path $SmokeRoot)

$Effective = (& $Control --data-dir $DataDir config effective | ConvertFrom-Json)
if ($Effective.server.bind -ne '127.0.0.1:9410') { throw 'compiled listener default is not loopback' }

$Doctor = (& $Control --data-dir $DataDir doctor | ConvertFrom-Json)
if ($Doctor.data_directory -ne 'owner_only') { throw 'doctor did not verify the owner-only data directory' }
if ($Doctor.storage -ne 'not_initialized') { throw 'doctor overstated storage readiness' }

$Process = Start-Process -FilePath $Daemon -ArgumentList @('--data-dir', $DataDir, '--bind', $BindAddress) -PassThru -RedirectStandardOutput $ServerOut -RedirectStandardError $ServerErr -Environment @{ RUST_LOG = 'info' }

try {
    $Started = $false
    foreach ($Attempt in 1..50) {
        if ($Process.HasExited) {
            throw "worldstreamd exited before probes; stderr: $(Get-Content $ServerErr -Raw)"
        }
        try {
            $Health = Invoke-WebRequest -Uri "$BaseUrl/healthz" -SkipHttpErrorCheck
            if ($Health.StatusCode -eq 200) {
                $Started = $true
                break
            }
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $Started) { throw 'worldstreamd did not become live within five seconds' }

    $Process.Refresh()
    if ($Process.HasExited) { throw "worldstreamd exited after probe; stderr: $(Get-Content $ServerErr -Raw)" }
    $StartupMarker = '"readiness":"ready"'
    if (-not (Select-String -LiteralPath $ServerOut -SimpleMatch $StartupMarker -Quiet)) {
        throw "worldstreamd did not emit this smoke run startup event; stdout: $(Get-Content $ServerOut -Raw)"
    }

    $Ready = Invoke-WebRequest -Uri "$BaseUrl/readyz" -SkipHttpErrorCheck
    $Version = Invoke-WebRequest -Uri "$BaseUrl/version" -SkipHttpErrorCheck
    if ($Ready.StatusCode -ne 200) { throw "readyz returned $($Ready.StatusCode), expected 200" }
    if ($Version.StatusCode -ne 200) { throw "version returned $($Version.StatusCode), expected 200" }

    $ReadyBody = $Ready.Content | ConvertFrom-Json
    $VersionBody = $Version.Content | ConvertFrom-Json
    if ($ReadyBody.status -ne 'ready') { throw 'readyz did not report the verified ready state' }
    if ($VersionBody.manifest.release_ready -ne $false) { throw 'checkout must not claim a qualified release' }
    if ($VersionBody.engine.status -ne 'verified') { throw 'version did not report the verified engine identity' }
    if (-not $VersionBody.engine.exact_identity.StartsWith('sqlite/')) { throw 'version did not report the bundled SQLite identity' }

    $Process.Refresh()
    if ($Process.HasExited) { throw "worldstreamd exited before smoke completion; stderr: $(Get-Content $ServerErr -Raw)" }
    if (-not (Select-String -LiteralPath $ServerOut -SimpleMatch $StartupMarker -Quiet)) {
        throw 'worldstreamd startup evidence disappeared before smoke completion'
    }
} finally {
    if (-not $Process.HasExited) {
        Stop-Process -Id $Process.Id -Force
        $Process.WaitForExit()
    }
    Remove-Item -LiteralPath $SmokeRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host 'WorldStream operator-shell smoke passed.'
