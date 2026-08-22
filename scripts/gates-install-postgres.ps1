$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$PostgresVersion = '17.11'
$PostgresArchive = 'postgresql-17.11-1-windows-x64-binaries.zip'
$PostgresSha256 = '6eabdf00d2893713b75db4336a23c3fdf505f056e217ec6e2e95d901750cfea3'
$PostgresUrl = "https://get.enterprisedb.com/postgresql/$PostgresArchive"

if (-not $env:RUNNER_TEMP -or -not $env:GITHUB_PATH -or -not $env:GITHUB_ENV) {
    throw 'hosted runner paths are required for pinned PostgreSQL provisioning'
}

$ArchivePath = Join-Path $env:RUNNER_TEMP $PostgresArchive
$InstallRoot = Join-Path $env:RUNNER_TEMP 'worldstream-postgresql-17.11'
$DataRoot = Join-Path $env:RUNNER_TEMP 'worldstream-postgresql-17.11-data'
$SecretRoot = Join-Path $env:RUNNER_TEMP 'worldstream-postgresql-17.11-secrets'
$LogPath = Join-Path $env:RUNNER_TEMP 'worldstream-postgresql-17.11.log'

Invoke-WebRequest -UseBasicParsing -Uri $PostgresUrl -OutFile $ArchivePath
$ObservedSha256 = (Get-FileHash -LiteralPath $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($ObservedSha256 -cne $PostgresSha256) {
    throw 'PostgreSQL 17.11 Windows archive digest mismatch'
}
New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
New-Item -ItemType Directory -Force -Path $SecretRoot | Out-Null
$CurrentIdentity = [Security.Principal.WindowsIdentity]::GetCurrent().Name

function Protect-WorldstreamPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [switch]$Directory
    )

    $Permission = if ($Directory) { '(OI)(CI)(F)' } else { '(F)' }
    icacls $Path /inheritance:r /grant:r "${CurrentIdentity}:$Permission" "*S-1-5-18:$Permission" "*S-1-5-32-544:$Permission" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'could not apply the protected owner/SYSTEM/Administrators PostgreSQL credential DACL'
    }
}
Protect-WorldstreamPath -Path $SecretRoot -Directory
Expand-Archive -LiteralPath $ArchivePath -DestinationPath $InstallRoot
$PostgresBin = Join-Path $InstallRoot 'pgsql/bin'
$Postgres = Join-Path $PostgresBin 'postgres.exe'
$Initdb = Join-Path $PostgresBin 'initdb.exe'
$PgCtl = Join-Path $PostgresBin 'pg_ctl.exe'
$PgIsReady = Join-Path $PostgresBin 'pg_isready.exe'
$Psql = Join-Path $PostgresBin 'psql.exe'
foreach ($Executable in @($Postgres, $Initdb, $PgCtl, $PgIsReady, $Psql)) {
    if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
        throw 'PostgreSQL 17.11 archive is missing a required executable'
    }
}
$ObservedVersion = (& $Postgres --version).Trim()
if ($LASTEXITCODE -ne 0 -or $ObservedVersion -cne "postgres (PostgreSQL) $PostgresVersion") {
    throw 'PostgreSQL server binary is not exact version 17.11'
}

$ExistingService = Get-Service -Name 'postgresql-x64-17' -ErrorAction SilentlyContinue
if ($ExistingService -and $ExistingService.Status -ne 'Stopped') {
    Stop-Service -Name $ExistingService.Name -Force
}

$RandomBytes = New-Object byte[] 24
$Random = [Security.Cryptography.RandomNumberGenerator]::Create()
try {
    $Random.GetBytes($RandomBytes)
}
finally {
    $Random.Dispose()
}
$PostgresPassword = -join ($RandomBytes | ForEach-Object { $_.ToString('x2') })
$BootstrapSecret = Join-Path $SecretRoot 'bootstrap-password'
$Utf8NoBom = New-Object Text.UTF8Encoding $false
[IO.File]::WriteAllText($BootstrapSecret, "$PostgresPassword`n", $Utf8NoBom)
Protect-WorldstreamPath -Path $BootstrapSecret

& $Initdb --pgdata $DataRoot --username postgres --pwfile $BootstrapSecret --auth-host scram-sha-256 --auth-local scram-sha-256 --encoding UTF8 --no-locale
if ($LASTEXITCODE -ne 0) {
    throw 'PostgreSQL 17.11 initdb failed'
}
& $PgCtl --pgdata $DataRoot --log $LogPath --options '-h 127.0.0.1 -p 5432' --wait start
if ($LASTEXITCODE -ne 0) {
    throw 'PostgreSQL 17.11 did not start'
}
& $PgIsReady --host 127.0.0.1 --port 5432 --username postgres --dbname postgres
if ($LASTEXITCODE -ne 0) {
    throw 'PostgreSQL 17.11 readiness probe failed'
}

$env:PGPASSWORD = $PostgresPassword
try {
    $ServerVersion = (& $Psql --host 127.0.0.1 --port 5432 --username postgres --dbname postgres --no-password --no-psqlrc --quiet --tuples-only --no-align --command 'SHOW server_version_num').Trim()
    $PsqlExitCode = $LASTEXITCODE
}
finally {
    Remove-Item Env:PGPASSWORD -ErrorAction SilentlyContinue
}
if ($PsqlExitCode -ne 0 -or $ServerVersion -cne '170011') {
    throw 'running PostgreSQL server is not exact version 17.11'
}

$PostgresDsnFile = Join-Path $SecretRoot 'worldstream-postgresql.dsn'
$PostgresScheme = 'postgresql'
$PostgresUser = 'postgres'
$PostgresDsn = $PostgresScheme + '://' + $PostgresUser + ':' + $PostgresPassword + '@127.0.0.1:5432/postgres'
[IO.File]::WriteAllText($PostgresDsnFile, "$PostgresDsn`n", $Utf8NoBom)
Protect-WorldstreamPath -Path $PostgresDsnFile
$PostgresBin | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append
"WORLDSTREAM_POSTGRES_DSN_FILE=$PostgresDsnFile" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
"WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE=$PostgresDsnFile" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append

Write-Host 'Provisioned byte-pinned PostgreSQL 17.11 with protected credential DACLs.'
