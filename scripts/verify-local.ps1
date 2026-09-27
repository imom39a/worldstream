$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($PSVersionTable.PSVersion -lt [version]'7.4') { throw 'PowerShell 7.4+ is required.' }
$PSNativeCommandUseErrorActionPreference = $true
Set-Location (Split-Path -Parent $PSScriptRoot)
$OfflineArgs = @()
if ($env:WORLDSTREAM_VERIFY_OFFLINE -eq '1') { $OfflineArgs = @('--offline') }
cargo fmt --all -- --check
cargo check @OfflineArgs --locked --all-targets
cargo build @OfflineArgs --locked
cargo test @OfflineArgs --locked -- --test-threads=1
cargo run @OfflineArgs --locked -p xtask -- compat verify
Write-Host 'Kernel and operator verification passed.'
