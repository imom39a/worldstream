$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($PSVersionTable.PSVersion -lt [version]'7.4') {
    throw 'PowerShell 7.4 or newer is required so native command failures fail verification.'
}
$PSNativeCommandUseErrorActionPreference = $true

$WorkspaceDir = Split-Path -Parent $PSScriptRoot
Set-Location $WorkspaceDir

$RequiredRust = '1.97.1'
$RequiredNode = '24.18.1'
$RequiredPython = '3.14.7'
$RequiredPnpm = '11.19.0'

foreach ($Tool in @('rustc', 'cargo', 'node', 'pnpm', 'uv', 'python')) {
    if (-not (Get-Command $Tool -ErrorAction SilentlyContinue)) {
        throw "Missing required tool: $Tool"
    }
}

$ActualRust = ((rustc --version) -split ' ')[1]
$ActualNode = node -p 'process.versions.node'
$ActualPnpm = pnpm --version

if ($ActualRust -ne $RequiredRust) { throw "Rust mismatch: need $RequiredRust, found $ActualRust" }
if ($ActualNode -ne $RequiredNode) { throw "Node mismatch: need $RequiredNode, found $ActualNode" }
if ($ActualPnpm -ne $RequiredPnpm) { throw "pnpm mismatch: need $RequiredPnpm, found $ActualPnpm" }

$Offline = $env:WORLDSTREAM_VERIFY_OFFLINE -eq '1'
$UvOffline = @()
$PnpmOffline = @()
$CargoOffline = @()
if ($Offline) {
    $UvOffline = @('--offline')
    $PnpmOffline = @('--offline')
    $CargoOffline = @('--offline')
}

uv sync @UvOffline --project sdk/python --locked --python $RequiredPython
$ActualPython = uv run @UvOffline --project sdk/python --python $RequiredPython python -c 'import platform; print(platform.python_version())'
if ($ActualPython -ne $RequiredPython) { throw "Python mismatch: need $RequiredPython, found $ActualPython" }

python scripts/verify-manifest.py
cargo fmt --all -- --check
cargo clippy @CargoOffline --workspace --all-targets --locked -- -D warnings
cargo build @CargoOffline --workspace --locked
cargo test @CargoOffline --workspace --locked
cargo run @CargoOffline --locked -p xtask -- compat verify

uv run @UvOffline --project sdk/python --python $RequiredPython ruff format --check
uv run @UvOffline --project sdk/python --python $RequiredPython ruff check
uv run @UvOffline --project sdk/python --python $RequiredPython pytest --ignore=target

pnpm install @PnpmOffline --frozen-lockfile
pnpm --dir sdk/typescript-client lint
pnpm --dir sdk/typescript-client test
pnpm --dir clients/agent-heist-web lint
pnpm --dir clients/agent-heist-web test
pnpm --dir clients/agent-heist-web build
pnpm --dir web/console lint
pnpm --dir web/console test
pnpm --dir web/console build

& "$PSScriptRoot/smoke-operator.ps1"
& "$PSScriptRoot/verify-agent-swarm-native.ps1"

Write-Host 'WorldStream bootstrap verification passed.'
