$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$PSNativeCommandUseErrorActionPreference = $true

if (-not $IsWindows) {
    throw 'Agent Swarm native smoke must run on Windows.'
}

$WorkspaceDir = Split-Path -Parent $PSScriptRoot
$SmokeRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("worldstream-agent-swarm-smoke-{0}" -f [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $SmokeRoot | Out-Null

try {
    Set-Location $WorkspaceDir
    cargo test --quiet --locked -p worldstream-agent-swarm --features managed-local-runtime
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    cargo test --quiet --locked -p worldstream-agent-swarm --features managed-local-runtime --test tui_pty
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    cargo build --quiet --locked -p worldstream-server -p worldstream-studio-supervisor --bins
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    cargo build --quiet --locked -p worldstream-agent-swarm --features managed-local-runtime --bins
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $Expected = '{"status":"ok","backend":"managed_local","rooms":2,"reopened":true,"reviewed_result":true,"reopened_reviewed_result":true,"late_output_fenced":true,"changed_input_revalidated":true,"review_correction_revalidated":true,"review_dispute_visible":true,"resource_conflict_reconciled":true,"human_steering_reassigned":true,"progress_review_claimed":true,"automatic_progress_review":true,"automatic_progress_review_waited_for_capacity":true,"bounded_realignments_escalated":true,"competing_claim":true,"duplicate_retry":true,"artifact_resolved":true,"coordinator_worker_contribution":true,"guarded_report_check":true,"reviewed_code_change":true,"code_change_conflict_preserved":true,"room_code_change_result":true,"native_shared_capacity":true,"native_progress_review_priority":true,"native_budget_pause":true,"native_effect_recovery":true}'
    $Actual = uv run --project sdk/python --python 3.14.7 python scripts/verify-agent-swarm-managed.py --workspace $WorkspaceDir --root $SmokeRoot
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($Actual -ne $Expected) {
        throw "Unexpected Agent Swarm smoke receipt: $Actual"
    }
    Write-Host 'Agent Swarm native Windows smoke passed.'
}
finally {
    $CleanupMarker = Join-Path $SmokeRoot 'managed-processes-stopped'
    if (Test-Path -LiteralPath $CleanupMarker) {
        Remove-Item -LiteralPath $SmokeRoot -Recurse -Force
    } else {
        Write-Warning "Agent Swarm smoke state retained for authenticated cleanup: $SmokeRoot"
    }
}
