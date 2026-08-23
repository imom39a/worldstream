$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$RequiredEnvironment = @(
    'GITHUB_WORKSPACE',
    'RUNNER_TEMP',
    'WORLDSTREAM_BUILD_REVISION',
    'WORLDSTREAM_PACKAGED_CTL',
    'WORLDSTREAM_POSTGRES_PASSWORD_FILE',
    'WORLDSTREAM_PG_DUMP',
    'WORLDSTREAM_PG_RESTORE',
    'WORLDSTREAM_PSQL'
)
foreach ($Name in $RequiredEnvironment) {
    if (-not [Environment]::GetEnvironmentVariable($Name)) {
        throw "hosted Windows native restore requires $Name"
    }
}

Set-Location -LiteralPath $env:GITHUB_WORKSPACE

$SourceDatabase = 'worldstream_native_source'
$TargetDatabase = 'worldstream_native_restore'
$AbortDatabase = 'worldstream_native_abort'
$AdminUser = 'postgres'
$RuntimeRole = 'worldstream_native_windows_runtime'
$TargetMarker = 'worldstream/native-postgres-disposable-target/v1'
$ReportPath = Join-Path $env:GITHUB_WORKSPACE 'reports/native-windows-postgres-restore.json'
$FixturePath = Join-Path $env:GITHUB_WORKSPACE 'reports/native-windows-transfer-seed.json'
$ReleasePython = (uv python find 3.14.7).Trim()

foreach ($Executable in @(
    $env:WORLDSTREAM_PACKAGED_CTL,
    $env:WORLDSTREAM_PG_DUMP,
    $env:WORLDSTREAM_PG_RESTORE,
    $env:WORLDSTREAM_PSQL,
    $ReleasePython
)) {
    if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
        throw 'hosted Windows native restore is missing an exact executable'
    }
}
if ((Test-Path -LiteralPath $ReportPath) -or (Test-Path -LiteralPath $FixturePath)) {
    throw 'hosted Windows native restore refuses to replace existing evidence'
}
if ($env:WORLDSTREAM_BUILD_REVISION -cnotmatch '^[0-9a-f]{40}$') {
    throw 'hosted Windows native restore source revision is malformed'
}

$AdminPassword = (Get-Content -LiteralPath $env:WORLDSTREAM_POSTGRES_PASSWORD_FILE -Raw).Trim()
if ($AdminPassword -cnotmatch '^[0-9a-f]{48}$') {
    throw 'hosted Windows PostgreSQL bootstrap credential is malformed'
}
$RandomBytes = New-Object byte[] 24
$Random = [Security.Cryptography.RandomNumberGenerator]::Create()
try {
    $Random.GetBytes($RandomBytes)
}
finally {
    $Random.Dispose()
}
$RuntimePassword = -join ($RandomBytes | ForEach-Object { $_.ToString('x2') })

$ProviderSystemIdentifier = $null
$SourceDatabaseOid = $null
$TargetDatabaseOid = $null
$AbortDatabaseOid = $null
$RuntimeRoleOid = $null
$ProviderCleanupComplete = $false
$PreserveRecovery = $false
$PassfileWriteStream = $null
$PassfileReadStream = $null
$PassfileScrubComplete = $false
$WorkParentHandle = $null

Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

public static class WorldstreamRetainedWindowsIdentity
{
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeFileTime
    {
        public uint Low;
        public uint High;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ByHandleFileInformation
    {
        public uint FileAttributes;
        public NativeFileTime CreationTime;
        public NativeFileTime LastAccessTime;
        public NativeFileTime LastWriteTime;
        public uint VolumeSerialNumber;
        public uint FileSizeHigh;
        public uint FileSizeLow;
        public uint NumberOfLinks;
        public uint FileIndexHigh;
        public uint FileIndexLow;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct FileId128
    {
        public ulong Low;
        public ulong High;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct FileIdInformation
    {
        public ulong VolumeSerialNumber;
        public FileId128 FileId;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(
        string fileName,
        uint desiredAccess,
        uint shareMode,
        IntPtr securityAttributes,
        uint creationDisposition,
        uint flagsAndAttributes,
        IntPtr templateFile
    );

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandle(
        SafeFileHandle handle,
        out ByHandleFileInformation information
    );

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandleEx(
        SafeFileHandle handle,
        int informationClass,
        out FileIdInformation information,
        uint bufferSize
    );

    public static SafeFileHandle OpenDirectory(string path)
    {
        SafeFileHandle handle = CreateFileW(
            path,
            0x80000000,
            0x00000001 | 0x00000002,
            IntPtr.Zero,
            3,
            0x02000000 | 0x00200000,
            IntPtr.Zero
        );
        if (handle.IsInvalid) {
            throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        return handle;
    }

    public static string Identity(SafeFileHandle handle)
    {
        ByHandleFileInformation information;
        if (!GetFileInformationByHandle(handle, out information)) {
            throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        FileIdInformation exact;
        if (!GetFileInformationByHandleEx(
            handle,
            18,
            out exact,
            (uint)Marshal.SizeOf<FileIdInformation>()
        )) {
            throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        string fileId = exact.FileId.High.ToString("x16") +
            exact.FileId.Low.ToString("x16");
        return exact.VolumeSerialNumber.ToString("x16") + "\t" + fileId +
            "\t" + information.NumberOfLinks.ToString();
    }
}
'@

function Invoke-WorldstreamProviderCleanup {
    if ($script:ProviderCleanupComplete -or -not $script:ProviderSystemIdentifier) {
        return
    }

    $SourceGuard = "NOT EXISTS (SELECT 1 FROM pg_database WHERE datname='$SourceDatabase')"
    $TargetGuard = "NOT EXISTS (SELECT 1 FROM pg_database WHERE datname='$TargetDatabase')"
    $AbortGuard = "NOT EXISTS (SELECT 1 FROM pg_database WHERE datname='$AbortDatabase')"
    $RoleGuard = "NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='$RuntimeRole')"
    $SourceDrop = ''
    $TargetDrop = ''
    $AbortDrop = ''
    $RoleDrop = ''
    if ($script:SourceDatabaseOid) {
        $SourceGuard = "($SourceGuard OR EXISTS (SELECT 1 FROM pg_database WHERE datname='$SourceDatabase' AND oid=$SourceDatabaseOid))"
        $SourceDrop = "DROP DATABASE IF EXISTS `"$SourceDatabase`" WITH (FORCE);"
    }
    if ($script:TargetDatabaseOid) {
        $TargetGuard = "($TargetGuard OR EXISTS (SELECT 1 FROM pg_database WHERE datname='$TargetDatabase' AND oid=$TargetDatabaseOid))"
        $TargetDrop = "DROP DATABASE IF EXISTS `"$TargetDatabase`" WITH (FORCE);"
    }
    if ($script:AbortDatabaseOid) {
        $AbortGuard = "($AbortGuard OR EXISTS (SELECT 1 FROM pg_database WHERE datname='$AbortDatabase' AND oid=$AbortDatabaseOid))"
        $AbortDrop = "DROP DATABASE IF EXISTS `"$AbortDatabase`" WITH (FORCE);"
    }
    if ($script:RuntimeRoleOid) {
        $RoleGuard = "($RoleGuard OR EXISTS (SELECT 1 FROM pg_roles WHERE rolname='$RuntimeRole' AND oid=$RuntimeRoleOid))"
        $RoleDrop = "DROP ROLE IF EXISTS `"$RuntimeRole`";"
    }

    $CleanupSql = @"
\set ON_ERROR_STOP on
SELECT (
  control.system_identifier::text = '$ProviderSystemIdentifier'
  AND $SourceGuard
  AND $TargetGuard
  AND $AbortGuard
  AND $RoleGuard
  AND NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname LIKE 'worldstream_restore_%')
) AS admitted
FROM pg_control_system() AS control
\gset
\if :admitted
$SourceDrop
$TargetDrop
$AbortDrop
$RoleDrop
\else
\quit 41
\endif
SELECT control.system_identifier::text,
  (SELECT count(*) FROM pg_database WHERE datname='$SourceDatabase'),
  (SELECT count(*) FROM pg_database WHERE datname='$TargetDatabase'),
  (SELECT count(*) FROM pg_database WHERE datname='$AbortDatabase'),
  (SELECT count(*) FROM pg_roles WHERE rolname='$RuntimeRole'),
  (SELECT count(*) FROM pg_roles WHERE rolname LIKE 'worldstream_restore_%')
FROM pg_control_system() AS control;
"@
    $env:PGPASSWORD = $AdminPassword
    try {
        $CleanupObservation = (($CleanupSql | & $env:WORLDSTREAM_PSQL --no-password --no-psqlrc `
            --host 127.0.0.1 --port 5432 --username $AdminUser --dbname postgres `
            --quiet --set ON_ERROR_STOP=1 --tuples-only --no-align `
            --field-separator "`t" --file -) | Out-String).Trim()
        if ($LASTEXITCODE -ne 0) {
            throw 'Windows provider fixture teardown refused its identity witness'
        }
    }
    finally {
        Remove-Item Env:PGPASSWORD -ErrorAction SilentlyContinue
    }
    $ExpectedCleanup = "$ProviderSystemIdentifier`t0`t0`t0`t0`t0"
    if ($CleanupObservation -cne $ExpectedCleanup) {
        throw 'Windows provider fixture teardown was not observed exactly'
    }
    $script:ProviderCleanupComplete = $true
}

function Clear-WorldstreamPassfile {
    if ($script:PassfileScrubComplete) {
        return
    }
    if ($null -ne $script:PassfileWriteStream) {
        $RetainedIdentity = [WorldstreamRetainedWindowsIdentity]::Identity(
            $script:PassfileWriteStream.SafeFileHandle
        ).Split("`t")
        if ($RetainedIdentity.Count -ne 3 `
            -or $RetainedIdentity[0] -cne $PassfileStorageId `
            -or $RetainedIdentity[1] -cne $PassfileFileId `
            -or $RetainedIdentity[2] -cne '1') {
            throw 'Windows operator passfile retained cleanup identity changed'
        }
        $script:PassfileWriteStream.SetLength(0)
        $script:PassfileWriteStream.Flush($true)
        $AfterRetainedIdentity = [WorldstreamRetainedWindowsIdentity]::Identity(
            $script:PassfileWriteStream.SafeFileHandle
        ).Split("`t")
        if ($script:PassfileWriteStream.Length -ne 0 `
            -or $AfterRetainedIdentity.Count -ne 3 `
            -or $AfterRetainedIdentity[0] -cne $RetainedIdentity[0] `
            -or $AfterRetainedIdentity[1] -cne $RetainedIdentity[1] `
            -or $AfterRetainedIdentity[2] -cne '1') {
            throw 'Windows operator passfile exact scrub failed'
        }
    }
    elseif ($null -ne $script:PassfileReadStream) {
        $CleanupStream = [IO.File]::Open(
            $Passfile,
            [IO.FileMode]::Open,
            [IO.FileAccess]::ReadWrite,
            [IO.FileShare]::Read
        )
        try {
            $CleanupIdentity = [WorldstreamRetainedWindowsIdentity]::Identity(
                $CleanupStream.SafeFileHandle
            ).Split("`t")
            if ($CleanupIdentity.Count -ne 3 `
                -or $CleanupIdentity[0] -cne $PassfileStorageId `
                -or $CleanupIdentity[1] -cne $PassfileFileId `
                -or $CleanupIdentity[2] -cne '1') {
                throw 'Windows operator passfile cleanup identity changed'
            }
            $CleanupStream.SetLength(0)
            $CleanupStream.Flush($true)
            $AfterCleanupIdentity = [WorldstreamRetainedWindowsIdentity]::Identity(
                $CleanupStream.SafeFileHandle
            ).Split("`t")
            if ($CleanupStream.Length -ne 0 `
                -or $script:PassfileReadStream.Length -ne 0 `
                -or $AfterCleanupIdentity.Count -ne 3 `
                -or $AfterCleanupIdentity[0] -cne $PassfileStorageId `
                -or $AfterCleanupIdentity[1] -cne $PassfileFileId `
                -or $AfterCleanupIdentity[2] -cne '1') {
                throw 'Windows operator passfile exact scrub failed'
            }
        }
        finally {
            $CleanupStream.Dispose()
        }
    }
    $script:PassfileScrubComplete = $true
}

function Close-WorldstreamPassfileHandles {
    foreach ($Name in @('PassfileReadStream', 'PassfileWriteStream')) {
        $Stream = Get-Variable -Name $Name -Scope Script -ValueOnly
        if ($null -ne $Stream) {
            $Stream.Dispose()
            Set-Variable -Name $Name -Scope Script -Value $null
        }
    }
}

try {

$env:PGPASSWORD = $AdminPassword
try {
    $ProviderSystemIdentifier = ((& $env:WORLDSTREAM_PSQL --no-password --no-psqlrc `
        --host 127.0.0.1 --port 5432 --username $AdminUser --dbname postgres `
        --quiet --set ON_ERROR_STOP=1 --tuples-only --no-align `
        --command 'SELECT system_identifier::text FROM pg_control_system()') | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $ProviderSystemIdentifier -cnotmatch '^[1-9][0-9]*$') {
        throw 'could not admit the exact Windows PostgreSQL provider'
    }

    & $env:WORLDSTREAM_PSQL --no-password --no-psqlrc --host 127.0.0.1 --port 5432 `
        --username $AdminUser --dbname postgres --quiet --set ON_ERROR_STOP=1 `
        --command "CREATE DATABASE $SourceDatabase"
    if ($LASTEXITCODE -ne 0) { throw 'could not create the isolated native restore source' }
    $SourceDatabaseOid = ((& $env:WORLDSTREAM_PSQL --no-password --no-psqlrc `
        --host 127.0.0.1 --port 5432 --username $AdminUser --dbname postgres `
        --quiet --set ON_ERROR_STOP=1 --tuples-only --no-align `
        --command "SELECT oid::text FROM pg_database WHERE datname='$SourceDatabase'") | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $SourceDatabaseOid -cnotmatch '^[1-9][0-9]*$') {
        throw 'could not retain the exact Windows source database identity'
    }
    & $env:WORLDSTREAM_PSQL --no-password --no-psqlrc --host 127.0.0.1 --port 5432 `
        --username $AdminUser --dbname postgres --quiet --set ON_ERROR_STOP=1 `
        --command "CREATE DATABASE $TargetDatabase"
    if ($LASTEXITCODE -ne 0) { throw 'could not create the disposable native restore target' }
    $TargetDatabaseOid = ((& $env:WORLDSTREAM_PSQL --no-password --no-psqlrc `
        --host 127.0.0.1 --port 5432 --username $AdminUser --dbname postgres `
        --quiet --set ON_ERROR_STOP=1 --tuples-only --no-align `
        --command "SELECT oid::text FROM pg_database WHERE datname='$TargetDatabase'") | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $TargetDatabaseOid -cnotmatch '^[1-9][0-9]*$') {
        throw 'could not retain the exact Windows target database identity'
    }
    & $env:WORLDSTREAM_PSQL --no-password --no-psqlrc --host 127.0.0.1 --port 5432 `
        --username $AdminUser --dbname postgres --quiet --set ON_ERROR_STOP=1 `
        --command "CREATE DATABASE $AbortDatabase"
    if ($LASTEXITCODE -ne 0) { throw 'could not create the transfer abort probe database' }
    $AbortDatabaseOid = ((& $env:WORLDSTREAM_PSQL --no-password --no-psqlrc `
        --host 127.0.0.1 --port 5432 --username $AdminUser --dbname postgres `
        --quiet --set ON_ERROR_STOP=1 --tuples-only --no-align `
        --command "SELECT oid::text FROM pg_database WHERE datname='$AbortDatabase'") | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $AbortDatabaseOid -cnotmatch '^[1-9][0-9]*$') {
        throw 'could not retain the exact Windows abort database identity'
    }

    $RuntimeSql = "CREATE ROLE $RuntimeRole LOGIN PASSWORD '$RuntimePassword' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION"
    & $env:WORLDSTREAM_PSQL --no-password --no-psqlrc --host 127.0.0.1 --port 5432 `
        --username $AdminUser --dbname postgres --quiet --set ON_ERROR_STOP=1 `
        --command $RuntimeSql
    if ($LASTEXITCODE -ne 0) { throw 'could not create the Windows live runtime role' }
    $RuntimeRoleOid = ((& $env:WORLDSTREAM_PSQL --no-password --no-psqlrc `
        --host 127.0.0.1 --port 5432 --username $AdminUser --dbname postgres `
        --quiet --set ON_ERROR_STOP=1 --tuples-only --no-align `
        --command "SELECT oid::text FROM pg_roles WHERE rolname='$RuntimeRole'") | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $RuntimeRoleOid -cnotmatch '^[1-9][0-9]*$') {
        throw 'could not retain the exact Windows runtime-role identity'
    }

    $PrivilegesSql = "ALTER DEFAULT PRIVILEGES FOR ROLE $AdminUser IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO $RuntimeRole; ALTER DEFAULT PRIVILEGES FOR ROLE $AdminUser IN SCHEMA public GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO $RuntimeRole; GRANT USAGE ON SCHEMA public TO $RuntimeRole"
    & $env:WORLDSTREAM_PSQL --no-password --no-psqlrc --host 127.0.0.1 --port 5432 `
        --username $AdminUser --dbname $SourceDatabase --quiet --set ON_ERROR_STOP=1 `
        --command $PrivilegesSql
    if ($LASTEXITCODE -ne 0) { throw 'could not grant Windows live runtime privileges' }

    $MarkerSql = "COMMENT ON DATABASE $TargetDatabase IS '$TargetMarker'; ALTER DATABASE $TargetDatabase CONNECTION LIMIT 0"
    & $env:WORLDSTREAM_PSQL --no-password --no-psqlrc --host 127.0.0.1 --port 5432 `
        --username $AdminUser --dbname postgres --quiet --set ON_ERROR_STOP=1 `
        --command $MarkerSql
    if ($LASTEXITCODE -ne 0) { throw 'could not seal the Windows disposable restore target' }

}
finally {
    Remove-Item Env:PGPASSWORD -ErrorAction SilentlyContinue
}

$CurrentIdentity = [Security.Principal.WindowsIdentity]::GetCurrent().Name
function Protect-WorldstreamPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [switch]$Directory
    )

    $Permission = if ($Directory) { '(OI)(CI)(F)' } else { '(F)' }
    & icacls.exe $Path /inheritance:r /grant:r "${CurrentIdentity}:$Permission" "*S-1-5-18:$Permission" "*S-1-5-32-544:$Permission" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'could not protect a Windows hosted native restore path'
    }
}

$SecretRoot = Join-Path $env:RUNNER_TEMP 'worldstream-native-restore-secrets'
$WorkParent = Join-Path $env:RUNNER_TEMP 'worldstream-native-restore-work'
$Passfile = Join-Path $SecretRoot 'operator.pgpass'
if ((Test-Path -LiteralPath $SecretRoot) -or (Test-Path -LiteralPath $WorkParent)) {
    throw 'hosted Windows native restore refuses a preexisting private root'
}

$Bash = (Get-Command bash.exe).Source
$WorkspacePosix = (& $Bash --noprofile --norc -c "cygpath -u '$env:GITHUB_WORKSPACE'").Trim()
if (-not $WorkspacePosix) {
    throw 'could not map the hosted Windows checkout for the transfer fixture'
}

$env:WORLDSTREAM_PG_TRANSFER_MODE = 'external'
$env:WORLDSTREAM_PG_TRANSFER_ADMIN_DSN = "host=127.0.0.1 port=5432 user=$AdminUser password=$AdminPassword dbname=$SourceDatabase"
$env:WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN = "host=127.0.0.1 port=5432 user=$RuntimeRole password=$RuntimePassword dbname=$SourceDatabase"
$env:WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN = "host=127.0.0.1 port=5432 user=$AdminUser password=$AdminPassword dbname=$AbortDatabase"
$env:WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE = $RuntimeRole
$env:WORLDSTREAM_PG_TRANSFER_SOURCE_REVISION = $env:WORLDSTREAM_BUILD_REVISION
try {
    & $Bash --noprofile --norc -c "cd '$WorkspacePosix' && scripts/postgres-transfer-smoke.sh --build-source --evidence reports/native-windows-transfer-seed.json"
    if ($LASTEXITCODE -ne 0) { throw 'Windows live source fixture construction failed' }
}
finally {
    Remove-Item Env:WORLDSTREAM_PG_TRANSFER_ADMIN_DSN -ErrorAction SilentlyContinue
    Remove-Item Env:WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN -ErrorAction SilentlyContinue
    Remove-Item Env:WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN -ErrorAction SilentlyContinue
    Remove-Item Env:WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE -ErrorAction SilentlyContinue
    Remove-Item Env:WORLDSTREAM_PG_TRANSFER_SOURCE_REVISION -ErrorAction SilentlyContinue
}

$Version = ([regex]::Match((Get-Content compatibility.toml -Raw), '(?m)^product\s*=\s*"([^"]+)"')).Groups[1].Value
$Archive = Join-Path $env:GITHUB_WORKSPACE "dist/windows/worldstream-$Version-windows-x64.zip"
if (-not (Test-Path -LiteralPath $Archive -PathType Leaf)) {
    throw 'hosted Windows native restore archive is absent'
}

New-Item -ItemType Directory -Path $SecretRoot | Out-Null
New-Item -ItemType Directory -Path $WorkParent | Out-Null
Protect-WorldstreamPath -Path $SecretRoot -Directory
Protect-WorldstreamPath -Path $WorkParent -Directory
$WorkParentHandle = [WorldstreamRetainedWindowsIdentity]::OpenDirectory($WorkParent)
$WorkParentIdentity = [WorldstreamRetainedWindowsIdentity]::Identity($WorkParentHandle).Split("`t")
if ($WorkParentIdentity.Count -ne 3) {
    throw 'could not retain the exact Windows hosted work parent identity'
}
$WorkParentStorageId = $WorkParentIdentity[0]
$WorkParentFileId = $WorkParentIdentity[1]
$PassfileWriteStream = [IO.FileStream]::new(
    $Passfile,
    [IO.FileMode]::CreateNew,
    [IO.FileAccess]::ReadWrite,
    [IO.FileShare]::ReadWrite
)
$Utf8NoBom = [Text.UTF8Encoding]::new($false)
$PassfileBytes = $Utf8NoBom.GetBytes(
    "127.0.0.1:5432:*:${AdminUser}:${AdminPassword}`n"
)
$PassfileWriteStream.Write($PassfileBytes, 0, $PassfileBytes.Length)
$PassfileWriteStream.Flush($true)
Protect-WorldstreamPath -Path $Passfile
$PassfileReadStream = [IO.File]::Open(
    $Passfile,
    [IO.FileMode]::Open,
    [IO.FileAccess]::Read,
    [IO.FileShare]::ReadWrite
)
if ($PassfileReadStream.Length -ne $PassfileBytes.Length) {
    throw 'Windows operator passfile changed during retained admission'
}
$PassfileIdentity = [WorldstreamRetainedWindowsIdentity]::Identity(
    $PassfileReadStream.SafeFileHandle
).Split("`t")
if ($PassfileIdentity.Count -ne 3 -or $PassfileIdentity[2] -cne '1') {
    throw 'could not retain the exact Windows operator passfile identity'
}
$PassfileStorageId = $PassfileIdentity[0]
$PassfileFileId = $PassfileIdentity[1]
$PassfileSize = $PassfileBytes.Length
$PassfileHasher = [Security.Cryptography.SHA256]::Create()
try {
    $PassfileSha256 = 'sha256:' + [Convert]::ToHexString(
        $PassfileHasher.ComputeHash($PassfileBytes)
    ).ToLowerInvariant()
}
finally {
    $PassfileHasher.Dispose()
}
$PassfileWriteStream.Dispose()
$PassfileWriteStream = $null
$PassfileBytes = $null

$HostedArguments = @(
    '-I',
    'scripts/postgres-native-restore-hosted-report.py',
    '--source', 'native-windows',
    '--package-report', 'reports/native-windows-package.json',
    '--runtime-report', 'reports/native-windows-package-runtime.json',
    '--artifact', $Archive,
    '--packaged-control', $env:WORLDSTREAM_PACKAGED_CTL,
    '--fixture-report', $FixturePath,
    '--source-host', '127.0.0.1',
    '--source-port', '5432',
    '--source-database', $SourceDatabase,
    '--source-username', $AdminUser,
    '--source-tls-mode', 'disable',
    '--target-host', '127.0.0.1',
    '--target-port', '5432',
    '--target-database', $TargetDatabase,
    '--target-username', $AdminUser,
    '--target-tls-mode', 'disable',
    '--passfile', $Passfile,
    '--passfile-storage-id', $PassfileStorageId,
    '--passfile-file-id', $PassfileFileId,
    '--passfile-size', $PassfileSize,
    '--passfile-sha256', $PassfileSha256,
    '--scrub-passfile-on-success',
    '--pg-dump', $env:WORLDSTREAM_PG_DUMP,
    '--pg-restore', $env:WORLDSTREAM_PG_RESTORE,
    '--psql', $env:WORLDSTREAM_PSQL,
    '--work-parent', $WorkParent,
    '--work-parent-storage-id', $WorkParentStorageId,
    '--work-parent-file-id', $WorkParentFileId,
    '--timeout-seconds', '300',
    '--output', $ReportPath
)
$PreserveRecovery = $true
& $ReleasePython @HostedArguments
$SupervisorExitCode = $LASTEXITCODE
if ($SupervisorExitCode -eq 0) {
    $PreserveRecovery = $false
}
elseif ($SupervisorExitCode -eq 43) {
    $PreserveRecovery = $false
    throw 'Windows hosted native restore failed after safe cleanup'
}
else {
    throw 'Windows hosted native restore supervisor failed closed'
}

$Report = Get-Content -LiteralPath $ReportPath -Raw | ConvertFrom-Json
if ($Report.schema -cne 'worldstream/hosted-native-postgres-restore-evidence/v3' `
    -or $Report.status -cne 'pass' `
    -or $Report.release_evidence `
    -or $Report.secrets_emitted `
    -or -not $Report.native_restore.native_witness_minted `
    -or $Report.cleanup.status -cne 'pass' `
    -or $Report.cleanup.target_database_after_drop -cne 'absent' `
    -or $Report.cleanup.operator_passfile_disposition -cne 'exact_retained_file_scrubbed_to_zero_length') {
    throw 'Windows hosted native restore report failed closed'
}

Invoke-WorldstreamProviderCleanup

$PassfileItem = Get-Item -LiteralPath $Passfile -Force
if ($PassfileItem.PSIsContainer `
    -or ($PassfileItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 `
    -or $PassfileItem.Length -ne 0) {
    throw 'Windows hosted native restore passfile was not scrubbed'
}
$PrivateRoots = @(Get-ChildItem -LiteralPath $WorkParent -Force)
$BindingRoots = @($PrivateRoots | Where-Object {
    $_.Name -ceq 'worldstream-native-platform-binding'
})
$ScrubbedRoots = @($PrivateRoots | Where-Object {
    $_.Name -cne 'worldstream-native-platform-binding'
})
if ($PrivateRoots.Count -ne 2 `
    -or $BindingRoots.Count -ne 1 `
    -or $ScrubbedRoots.Count -ne 1 `
    -or -not $BindingRoots[0].PSIsContainer `
    -or -not $ScrubbedRoots[0].PSIsContainer `
    -or ($BindingRoots[0].Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 `
    -or ($ScrubbedRoots[0].Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 `
    -or $Report.private_binding.root_name -cne $BindingRoots[0].Name) {
    throw 'Windows hosted native restore work root changed'
}
$PrivateArtifacts = @(Get-ChildItem -LiteralPath $ScrubbedRoots[0].FullName -Force)
if ($PrivateArtifacts.Count -ne $Report.cleanup.private_artifact_placeholder_count `
    -or @($PrivateArtifacts | Where-Object {
        $_.PSIsContainer `
        -or ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 `
        -or $_.Length -ne 0
    }).Count -ne 0) {
    throw 'Windows hosted native restore private artifacts were not scrubbed'
}
$BindingArtifacts = @(Get-ChildItem -LiteralPath $BindingRoots[0].FullName -Force)
$ExpectedBindingNames = @(
    $Report.private_binding.files.PSObject.Properties | ForEach-Object {
        $_.Value.name
    }
)
if ($BindingArtifacts.Count -ne 5 `
    -or (Compare-Object -CaseSensitive `
        -ReferenceObject @($ExpectedBindingNames | Sort-Object) `
        -DifferenceObject @($BindingArtifacts.Name | Sort-Object)).Count -ne 0 `
    -or @($BindingArtifacts | Where-Object {
        $_.PSIsContainer `
        -or ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 `
        -or $_.Length -le 0
    }).Count -ne 0) {
    throw 'Windows hosted native restore private binding is incomplete'
}

Clear-WorldstreamPassfile
}
finally {
    $CleanupFailure = $null
    Remove-Item Env:PGPASSWORD -ErrorAction SilentlyContinue
    Remove-Item Env:WORLDSTREAM_PG_TRANSFER_ADMIN_DSN -ErrorAction SilentlyContinue
    Remove-Item Env:WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN -ErrorAction SilentlyContinue
    Remove-Item Env:WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN -ErrorAction SilentlyContinue
    Remove-Item Env:WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE -ErrorAction SilentlyContinue
    Remove-Item Env:WORLDSTREAM_PG_TRANSFER_SOURCE_REVISION -ErrorAction SilentlyContinue
    if (-not $PreserveRecovery) {
        try {
            Invoke-WorldstreamProviderCleanup
        }
        catch {
            $CleanupFailure = $_
        }
        try {
            Clear-WorldstreamPassfile
        }
        catch {
            if ($null -eq $CleanupFailure) {
                $CleanupFailure = $_
            }
        }
    }
    Close-WorldstreamPassfileHandles
    if ($null -ne $WorkParentHandle) {
        $WorkParentHandle.Dispose()
        $WorkParentHandle = $null
    }
    $AdminPassword = $null
    $RuntimePassword = $null
    if ($null -ne $CleanupFailure) {
        throw $CleanupFailure
    }
}

Write-Host 'Windows hosted PostgreSQL 17.11 native backup/restore/recovery passed.'
