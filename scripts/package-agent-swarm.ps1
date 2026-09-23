param(
    [Parameter(Mandatory = $true)][string]$PackPath,
    [Parameter(Mandatory = $true)][string]$PackVersion,
    [Parameter(Mandatory = $true)][string]$PackDigest,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$PSNativeCommandUseErrorActionPreference = $true

if (-not $IsWindows -or $env:PROCESSOR_ARCHITECTURE -notin @('AMD64', 'x86_64')) {
    throw 'Agent Swarm Windows archives must be built on native Windows x64.'
}

$WorkspaceDir = Split-Path -Parent $PSScriptRoot
Set-Location $WorkspaceDir
cargo build --release --locked -p worldstream-server -p worldstream-studio-supervisor --bins
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo build --release --locked -p worldstream-agent-swarm --features managed-local-runtime --bins
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$Version = uv run --project sdk/python --python 3.14.7 python -c 'import tomllib; print(tomllib.load(open("compatibility.toml", "rb"))["contracts"]["product"])'
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
uv run --project sdk/python --python 3.14.7 python scripts/agent-swarm-package.py build `
    --target windows-x64 `
    --binary-dir target/release `
    --pack $PackPath `
    --pack-version $PackVersion `
    --pack-digest $PackDigest `
    --version $Version `
    --output $OutputDirectory
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
