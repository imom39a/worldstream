#!/bin/bash
set -euo pipefail

# Provider-native PostgreSQL 17.11 custom-format dump/restore evidence.
# This lane owns only its disposable Docker containers and temporary dump.
# A local result is operational evidence only and always has release_evidence=false.

readonly EXIT_PASS=0
readonly EXIT_UNAVAILABLE=10
readonly EXIT_CONFIGURATION=12
readonly EXIT_INCOMPLETE=13
readonly EXIT_CLEANUP=14

usage() {
  printf '%s\n' \
    "Usage: scripts/postgres-native-restore-smoke.sh [--evidence PATH]" \
    "       [--source-host HOST --source-port PORT --source-tls-mode require|disable]" \
    "       [--target-host HOST --target-port PORT --target-tls-mode require|disable]" \
    "Uses digest-pinned PostgreSQL 17.11 Docker containers by default." \
    "External targets must be empty/non-serving and carry database comment:" \
    "worldstream/native-postgres-disposable-target/v1"
}

# Help is deliberately handled before Python discovery or temporary-root
# creation. Besides remaining provider-independent, this makes --help a
# side-effect-free operation even when TMPDIR is attacker-observable.
for early_argument in "$@"; do
  case "$early_argument" in
    --help|-h)
      usage
      exit "$EXIT_PASS"
      ;;
  esac
done

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
invocation_dir="$PWD"
source_host="${WORLDSTREAM_NATIVE_PG_SOURCE_HOST:-}"
source_port="${WORLDSTREAM_NATIVE_PG_SOURCE_PORT:-}"
source_db="${WORLDSTREAM_NATIVE_PG_SOURCE_DB:-worldstream}"
source_user="${WORLDSTREAM_NATIVE_PG_SOURCE_USER:-postgres}"
source_tls_mode="${WORLDSTREAM_NATIVE_PG_SOURCE_TLS_MODE:-require}"
target_host="${WORLDSTREAM_NATIVE_PG_TARGET_HOST:-}"
target_port="${WORLDSTREAM_NATIVE_PG_TARGET_PORT:-}"
target_db="${WORLDSTREAM_NATIVE_PG_TARGET_DB:-worldstream}"
target_user="${WORLDSTREAM_NATIVE_PG_TARGET_USER:-postgres}"
target_tls_mode="${WORLDSTREAM_NATIVE_PG_TARGET_TLS_MODE:-require}"
passfile="${WORLDSTREAM_NATIVE_PG_PGPASSFILE:-}"
docker_bin="${WORLDSTREAM_NATIVE_PG_DOCKER:-}"
cargo_bin="${WORLDSTREAM_NATIVE_PG_CARGO:-}"
python_bin="${WORLDSTREAM_NATIVE_PG_PYTHON:-}"
pg_dump_bin="${WORLDSTREAM_NATIVE_PG_DUMP:-}"
pg_restore_bin="${WORLDSTREAM_NATIVE_PG_RESTORE:-}"
psql_bin="${WORLDSTREAM_NATIVE_PG_PSQL:-}"
transfer_smoke_bin="${WORLDSTREAM_NATIVE_PG_TRANSFER_SMOKE:-$root_dir/scripts/postgres-transfer-smoke.sh}"
evidence_file="${WORLDSTREAM_NATIVE_PG_EVIDENCE_FILE:-}"
temp_root=""
source_container=""
target_container=""
source_container_id=""
target_container_id=""
cleanup_status="not_started"
cleanup_completed=0
owned_docker=0
owned_pg_dump_fd=""
owned_pg_restore_fd=""
owned_psql_fd=""
authority_python="${WORLDSTREAM_NATIVE_PG_PYTHON:-}"
if [[ -z "$authority_python" || ! -x "$authority_python" ]]; then
  if [[ -x /usr/bin/python3 ]]; then
    authority_python=/usr/bin/python3
  else
    authority_python="$(command -v python3 2>/dev/null || command -v python 2>/dev/null || true)"
  fi
fi
if [[ -z "$authority_python" || ! -x "$authority_python" ]]; then
  printf '%s\n' '{"exit_code":10,"live_restore_concurrency":null,"native_restore":null,"reason":"python_unavailable_for_temporary_authority","release_evidence":false,"schema":"worldstream/native-postgres-restore-smoke-evidence/v1","secrets_emitted":false,"status":"unavailable","target_isolated":false,"target_published":false}'
  exit "$EXIT_UNAVAILABLE"
fi
authority_os_name="$("$authority_python" -c 'import os; print(os.name)')"
temp_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-native-pg.XXXXXX")"
temp_root_path="$temp_root"
chmod 700 "$temp_root_path"
authority_root="$temp_root_path"
temp_root_fd=-1
portable_windows_authority="${WORLDSTREAM_NATIVE_PG_TEST_PORTABLE_WINDOWS_AUTHORITY:-0}"
windows_keeper_started=0

artifact_names=()
artifact_kinds=()
artifact_storage_ids=()
artifact_file_ids=()
artifact_plan=(
  docker.env pgpass mount-authority restore-input.dump restore-concurrency.observation
  target-marker.stdout target-marker.stderr runtime-role.stdout runtime-role.stderr
  transfer-abort-database.stdout transfer-abort-database.stderr
  transfer-admin.dsn transfer-runtime.dsn transfer-abort-admin.dsn
  source-seed.stdout source-seed.stderr source-prepare.stdout source-prepare.stderr
  target-seal.stderr target-restore-role-cleanup.stderr driver.stderr
  pg_dump pg_restore psql
)
artifact_keeper_started=0
next_artifact_fd=20

identity_for_path() {
  "$authority_python" - "$1" <<'PY'
import ctypes
import os
import sys

path = os.path.abspath(sys.argv[1])
if os.name != "nt":
    value = os.lstat(path)
    print(f"{value.st_dev:016x} {value.st_ino:032x}")
    raise SystemExit(0)

from ctypes import wintypes

class FILE_ID_128(ctypes.Structure):
    _fields_ = [("Low", ctypes.c_ulonglong), ("High", ctypes.c_ulonglong)]

class FILE_ID_INFO(ctypes.Structure):
    _fields_ = [("VolumeSerialNumber", ctypes.c_ulonglong), ("FileId", FILE_ID_128)]

CreateFileW = ctypes.windll.kernel32.CreateFileW
CreateFileW.argtypes = [
    wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
    wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE,
]
CreateFileW.restype = wintypes.HANDLE
GetFileInformationByHandleEx = ctypes.windll.kernel32.GetFileInformationByHandleEx
GetFileInformationByHandleEx.argtypes = [
    wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD,
]
GetFileInformationByHandleEx.restype = wintypes.BOOL
CloseHandle = ctypes.windll.kernel32.CloseHandle
CloseHandle.argtypes = [wintypes.HANDLE]
handle = CreateFileW(
    path,
    0x80000000,
    0x00000001 | 0x00000002,
    None,
    3,
    0x02000000 | 0x00200000,
    None,
)
if handle == wintypes.HANDLE(-1).value:
    raise ctypes.WinError()
try:
    info = FILE_ID_INFO()
    if not GetFileInformationByHandleEx(handle, 18, ctypes.byref(info), ctypes.sizeof(info)):
        raise ctypes.WinError()
    print(
        f"{info.VolumeSerialNumber:016x}"
        f" {info.FileId.High:016x}{info.FileId.Low:016x}"
    )
finally:
    CloseHandle(handle)
PY
}

if [[ "${OS:-}" == "Windows_NT" && "$portable_windows_authority" == "1" \
    && "$authority_os_name" != "nt" ]]; then
  # Executable portability regression: take the Windows full-path helper route
  # on a POSIX host while retaining FD 9 solely as the simulation root guard.
  # Native Windows never enters this explicitly test-only branch.
  readonly temp_root_fd=9
  if ! { exec 9<"$temp_root_path"; }; then
    printf '%s\n' 'native PostgreSQL restore: failed to retain portable Windows test authority' >&2
    exit "$EXIT_CONFIGURATION"
  fi
elif [[ "${OS:-}" == "Windows_NT" ]]; then
  authority_root="$(cygpath -w "$temp_root_path")"
  if ! powershell.exe -NoProfile -NonInteractive -Command '
    param([string]$Path)
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent().Name
    & icacls.exe $Path /inheritance:r /setowner $identity /grant:r "${identity}:(OI)(CI)F" "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" | Out-Null
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
  ' "$authority_root"; then
    printf '%s\n' 'native PostgreSQL restore: protected Windows temporary directory unavailable' >&2
    exit "$EXIT_CONFIGURATION"
  fi
  # Keep one native directory handle open without FILE_SHARE_DELETE for the
  # whole operation. Docker's Windows path source therefore cannot be renamed
  # to another directory between admission and daemon mount resolution.
  exec 8< <(
    "$authority_python" -c '
import ctypes, os, sys, time
from ctypes import wintypes
p = sys.argv[1]
c = ctypes.windll.kernel32.CreateFileW
c.argtypes = [wintypes.LPCWSTR,wintypes.DWORD,wintypes.DWORD,wintypes.LPVOID,wintypes.DWORD,wintypes.DWORD,wintypes.HANDLE]
c.restype = wintypes.HANDLE
h = c(p,0x80000000,0x00000001|0x00000002,None,3,0x02000000|0x00200000,None)
if h == wintypes.HANDLE(-1).value: raise ctypes.WinError()
print("ready", flush=True)
while True:
    try:
        os.write(1, b".")
    except BrokenPipeError:
        devnull = os.open(os.devnull, os.O_WRONLY)
        os.dup2(devnull, 1)
        os.close(devnull)
        break
    time.sleep(0.5)
ctypes.windll.kernel32.CloseHandle(h)
' "$authority_root" &
    windows_keeper_worker_pid=$!
    wait "$windows_keeper_worker_pid"
  )
  windows_keeper_started=1
  if ! read -r keeper_ready <&8 || [[ "$keeper_ready" != "ready" ]]; then
    printf '%s\n' 'native PostgreSQL restore: failed to retain Windows temporary directory authority' >&2
    exit "$EXIT_CONFIGURATION"
  fi
else
  readonly temp_root_fd=9
  if ! { exec 9<"$temp_root_path"; }; then
    printf '%s\n' 'native PostgreSQL restore: failed to retain temporary directory authority' >&2
    exit "$EXIT_CONFIGURATION"
  fi
fi

if ! read -r temp_root_storage_id temp_root_file_id < <(identity_for_path "$authority_root"); then
  printf '%s\n' 'native PostgreSQL restore: failed to identify temporary directory authority' >&2
  exit "$EXIT_CONFIGURATION"
fi

# Every local temporary artifact is addressed relative to the process cwd.
# Once cwd is admitted against FD 9, renaming or replacing the mktemp pathname
# cannot redirect later redirections, wrapper creation, or tool outputs.
if [[ -n "${WORLDSTREAM_NATIVE_PG_TEST_AFTER_TEMP_ROOT_ADMISSION:-}" ]]; then
  "${WORLDSTREAM_NATIVE_PG_TEST_AFTER_TEMP_ROOT_ADMISSION}" "$temp_root_path"
fi
retained_cwd_matches() {
  "$authority_python" - "$temp_root_fd" "$authority_root" \
    "$portable_windows_authority" <<'PY' >/dev/null 2>&1
import os
import sys

if os.name == "nt" or sys.argv[3] == "1":
    retained = os.stat(sys.argv[2], follow_symlinks=False)
else:
    retained = os.fstat(int(sys.argv[1]))
current = os.stat(".", follow_symlinks=False)
raise SystemExit(
    (retained.st_dev, retained.st_ino) != (current.st_dev, current.st_ino)
)
PY
}
if ! cd -- "$temp_root_path" || ! retained_cwd_matches; then
  printf '%s\n' 'native PostgreSQL restore: temporary directory identity changed before use' >&2
  exit "$EXIT_CONFIGURATION"
fi
temp_root="."

scrub_retained_temp_root() {
  local records=()
  local index
  for ((index = 0; index < ${#artifact_names[@]}; index++)); do
    records+=("${artifact_names[$index]}|${artifact_kinds[$index]}|${artifact_storage_ids[$index]}|${artifact_file_ids[$index]}")
  done
  "$authority_python" - "$temp_root_fd" "$authority_root" \
    "$temp_root_storage_id" "$temp_root_file_id" \
    "$portable_windows_authority" \
    ${records[@]+"${records[@]}"} <<'PY'
import ctypes
import os
import re
import stat
import subprocess
import sys

ROOT_FD = int(sys.argv[1])
ROOT_PATH = sys.argv[2]
ROOT_ID = (sys.argv[3], sys.argv[4])
PATH_AUTHORITY = os.name == "nt" or sys.argv[5] == "1"
RECORDS = [tuple(value.split("|", 3)) for value in sys.argv[6:]]
SAFE_EMPTY = re.compile(r"[.]worldstream_[A-Za-z0-9_.-]{1,160}")


def unix_identity(value: os.stat_result) -> tuple[str, str]:
    return f"{value.st_dev:016x}", f"{value.st_ino:032x}"


if os.name == "nt":
    import msvcrt
    from ctypes import wintypes

    class FILE_ID_128(ctypes.Structure):
        _fields_ = [("Low", ctypes.c_ulonglong), ("High", ctypes.c_ulonglong)]

    class FILE_ID_INFO(ctypes.Structure):
        _fields_ = [("VolumeSerialNumber", ctypes.c_ulonglong), ("FileId", FILE_ID_128)]

    get_info = ctypes.windll.kernel32.GetFileInformationByHandleEx
    get_info.argtypes = [wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD]
    get_info.restype = wintypes.BOOL
    create_file = ctypes.windll.kernel32.CreateFileW
    create_file.argtypes = [
        wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
        wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE,
    ]
    create_file.restype = wintypes.HANDLE
    close_handle = ctypes.windll.kernel32.CloseHandle
    close_handle.argtypes = [wintypes.HANDLE]
    close_handle.restype = wintypes.BOOL

    def handle_identity(fd: int) -> tuple[str, str]:
        info = FILE_ID_INFO()
        handle = wintypes.HANDLE(msvcrt.get_osfhandle(fd))
        if not get_info(handle, 18, ctypes.byref(info), ctypes.sizeof(info)):
            raise ctypes.WinError()
        return (
            f"{info.VolumeSerialNumber:016x}",
            f"{info.FileId.High:016x}{info.FileId.Low:016x}",
        )

    def path_identity(path: str) -> tuple[str, str]:
        handle = create_file(
            path,
            0x00000080,  # FILE_READ_ATTRIBUTES
            0x00000001 | 0x00000002 | 0x00000004,
            None,
            3,  # OPEN_EXISTING
            0x02000000 | 0x00200000,  # BACKUP_SEMANTICS | OPEN_REPARSE_POINT
            None,
        )
        if handle == wintypes.HANDLE(-1).value:
            raise ctypes.WinError()
        try:
            info = FILE_ID_INFO()
            if not get_info(handle, 18, ctypes.byref(info), ctypes.sizeof(info)):
                raise ctypes.WinError()
            return (
                f"{info.VolumeSerialNumber:016x}",
                f"{info.FileId.High:016x}{info.FileId.Low:016x}",
            )
        finally:
            close_handle(handle)

    def named_stat(name: str) -> os.stat_result:
        return os.lstat(os.path.join(ROOT_PATH, name))

    def open_named(name: str, kind: str) -> int:
        path = os.path.join(ROOT_PATH, name)
        if kind != "directory":
            return os.open(path, os.O_RDWR | getattr(os, "O_BINARY", 0))
        handle = create_file(
            path,
            0x00000080,
            0x00000001 | 0x00000002 | 0x00000004,
            None,
            3,
            0x02000000 | 0x00200000,
            None,
        )
        if handle == wintypes.HANDLE(-1).value:
            raise ctypes.WinError()
        try:
            return msvcrt.open_osfhandle(handle, os.O_RDONLY)
        except BaseException:
            close_handle(handle)
            raise
elif PATH_AUTHORITY:
    def handle_identity(fd: int) -> tuple[str, str]:
        return unix_identity(os.fstat(fd))

    def path_identity(path: str) -> tuple[str, str]:
        return unix_identity(os.lstat(path))

    def named_stat(name: str) -> os.stat_result:
        return os.lstat(os.path.join(ROOT_PATH, name))

    def open_named(name: str, kind: str) -> int:
        flags = os.O_RDONLY if kind == "directory" else os.O_RDWR
        flags |= os.O_NOFOLLOW | os.O_CLOEXEC
        if kind == "directory":
            flags |= os.O_DIRECTORY
        return os.open(os.path.join(ROOT_PATH, name), flags)
else:
    def handle_identity(fd: int) -> tuple[str, str]:
        return unix_identity(os.fstat(fd))

    def path_identity(path: str) -> tuple[str, str]:
        return unix_identity(os.lstat(path))

    def named_stat(name: str) -> os.stat_result:
        return os.stat(name, dir_fd=ROOT_FD, follow_symlinks=False)

    def open_named(name: str, kind: str) -> int:
        flags = os.O_RDONLY if kind == "directory" else os.O_RDWR
        flags |= os.O_NOFOLLOW | os.O_CLOEXEC
        if kind == "directory":
            flags |= os.O_DIRECTORY
        return os.open(name, flags, dir_fd=ROOT_FD)


root = os.stat(ROOT_PATH, follow_symlinks=False) if PATH_AUTHORITY else os.fstat(ROOT_FD)
if not stat.S_ISDIR(root.st_mode):
    raise RuntimeError("retained temporary root is not a directory")
if path_identity(ROOT_PATH) != ROOT_ID:
    raise RuntimeError("temporary root pathname no longer names admitted authority")
if os.name != "nt" and (root.st_uid != os.geteuid() or stat.S_IMODE(root.st_mode) & 0o077):
    raise RuntimeError("retained temporary root is not owner-only")

expected_names = {record[0] for record in RECORDS}
if len(expected_names) != len(RECORDS):
    raise RuntimeError("temporary cleanup manifest has duplicate names")
actual_names = sorted(os.listdir(ROOT_PATH if PATH_AUTHORITY else ROOT_FD))
unknown_names = [name for name in actual_names if name not in expected_names]

# Phase one opens and validates every expected object and every narrowly
# admitted empty producer placeholder. Nothing is mutated until every retained
# handle and pathname has been rechecked after all opens. Unknown placeholders
# are never scrubbed; holding them only prevents a check/use substitution from
# being mistaken for a successful cleanup.
opened: list[tuple[str, str, tuple[str, str], int]] = []
unknown_opened: list[tuple[str, tuple[str, str], int]] = []
try:
    for name in unknown_names:
        value = named_stat(name)
        if (
            not SAFE_EMPTY.fullmatch(name)
            or not stat.S_ISREG(value.st_mode)
            or value.st_nlink != 1
            or value.st_size != 0
        ):
            raise RuntimeError(
                "unmanifested temporary artifact is not an inert placeholder"
            )
        initial_identity = path_identity(os.path.join(ROOT_PATH, name))
        fd = open_named(name, "file")
        held = os.fstat(fd)
        if (
            handle_identity(fd) != initial_identity
            or not stat.S_ISREG(held.st_mode)
            or held.st_nlink != 1
            or held.st_size != 0
        ):
            os.close(fd)
            raise RuntimeError(
                "unmanifested temporary artifact changed during admission"
            )
        if os.name != "nt" and (
            held.st_uid != os.geteuid() or stat.S_IMODE(held.st_mode) & 0o077
        ):
            os.close(fd)
            raise RuntimeError(
                "unmanifested temporary artifact is not owner-only"
            )
        unknown_opened.append((name, initial_identity, fd))

    cleanup_admission_hook = os.environ.get(
        "WORLDSTREAM_NATIVE_PG_TEST_AFTER_UNKNOWN_CLEANUP_ADMISSION"
    )
    if cleanup_admission_hook:
        if not unknown_names:
            raise RuntimeError("cleanup admission hook requires an unknown placeholder")
        subprocess.run(
            [cleanup_admission_hook, ROOT_PATH, *unknown_names],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=True,
        )

    for name, kind, storage_id, file_id in RECORDS:
        if not name or "/" in name or "\\" in name or name in (".", ".."):
            raise RuntimeError("temporary cleanup manifest name is unsafe")
        expected = (storage_id, file_id)
        value = named_stat(name)
        if kind == "file":
            if not stat.S_ISREG(value.st_mode) or value.st_nlink != 1:
                raise RuntimeError("manifested temporary file is unsafe")
        elif kind == "directory":
            if not stat.S_ISDIR(value.st_mode):
                raise RuntimeError("manifested temporary directory is unsafe")
        else:
            raise RuntimeError("temporary cleanup manifest kind is invalid")
        fd = open_named(name, kind)
        if handle_identity(fd) != expected:
            os.close(fd)
            raise RuntimeError("temporary artifact identity changed before cleanup")
        if os.name != "nt":
            held = os.fstat(fd)
            if held.st_uid != os.geteuid() or stat.S_IMODE(held.st_mode) & 0o077:
                os.close(fd)
                raise RuntimeError("temporary artifact is not owner-only")
        opened.append((name, kind, expected, fd))

    if sorted(os.listdir(ROOT_PATH if PATH_AUTHORITY else ROOT_FD)) != actual_names:
        raise RuntimeError("temporary entries changed during cleanup preflight")
    for name, expected, fd in unknown_opened:
        held = os.fstat(fd)
        if (
            handle_identity(fd) != expected
            or not stat.S_ISREG(held.st_mode)
            or held.st_nlink != 1
            or held.st_size != 0
            or path_identity(os.path.join(ROOT_PATH, name)) != expected
        ):
            raise RuntimeError(
                "unmanifested temporary artifact changed before mutation"
            )
    for name, _, expected, _ in opened:
        if path_identity(os.path.join(ROOT_PATH, name)) != expected:
            raise RuntimeError("temporary artifact pathname changed before mutation")

    # Phase two mutates only the exact handles retained by phase one. Named
    # files are never opened after mutation starts, and directories are kept.
    for _, kind, expected, fd in opened:
        if kind != "file":
            continue
        os.ftruncate(fd, 0)
        os.fsync(fd)
        after = os.fstat(fd)
        if after.st_nlink != 1 or after.st_size != 0 or handle_identity(fd) != expected:
            raise RuntimeError("exact-handle temporary scrub failed")
finally:
    for _, _, _, fd in reversed(opened):
        os.close(fd)
    for _, _, fd in reversed(unknown_opened):
        os.close(fd)
PY
}

cleanup_resources() {
  if [[ "$cleanup_completed" -eq 1 ]]; then
    [[ "$cleanup_status" == "pass" ]]
    return
  fi
  local failed=0
  if [[ "$owned_docker" -eq 1 ]]; then
    [[ -z "$source_container_id" ]] || "$docker_bin" rm -f "$source_container_id" >/dev/null 2>&1 || failed=1
    [[ -z "$target_container_id" ]] || "$docker_bin" rm -f "$target_container_id" >/dev/null 2>&1 || failed=1
  fi
  if [[ -n "${WORLDSTREAM_NATIVE_PG_TEST_BEFORE_TEMP_CLEANUP:-}" ]]; then
    "${WORLDSTREAM_NATIVE_PG_TEST_BEFORE_TEMP_CLEANUP}" "$temp_root_path" || failed=1
  fi
  # The full manifest is preflighted before the first truncate. Only exact
  # retained identities are scrubbed; protected empty placeholders and the
  # owner-only directory tree are intentionally retained.
  if [[ "$artifact_keeper_started" -ne 1 ]] || ! keeper_pipe_alive 7; then
    failed=1
  else
    scrub_retained_temp_root || failed=1
  fi
  local wrapper_fd_variable wrapper_fd
  for wrapper_fd_variable in owned_pg_dump_fd owned_pg_restore_fd owned_psql_fd; do
    wrapper_fd="${!wrapper_fd_variable}"
    if [[ -n "$wrapper_fd" ]]; then
      close_artifact_fd "$wrapper_fd" || failed=1
      printf -v "$wrapper_fd_variable" '%s' ""
    fi
  done
  if [[ "$artifact_keeper_started" -eq 1 ]]; then
    keeper_pipe_alive 7 || failed=1
    exec 7<&- || failed=1
    artifact_keeper_started=0
  fi
  if [[ "${OS:-}" == "Windows_NT" ]]; then
    if [[ "$windows_keeper_started" -eq 1 ]]; then
      keeper_pipe_alive 8 || failed=1
      exec 8<&- || failed=1
      windows_keeper_started=0
    elif [[ "$temp_root_fd" == "9" ]]; then
      exec 9<&- || failed=1
    fi
  else
    exec 9<&- || failed=1
  fi
  if [[ "${WORLDSTREAM_NATIVE_PG_TEST_CLEANUP_FAILURE:-0}" == "1" ]]; then
    failed=1
  fi
  cleanup_completed=1
  if [[ "$failed" -eq 0 ]]; then cleanup_status="pass"; else cleanup_status="failed"; fi
  [[ "$failed" -eq 0 ]]
}
trap 'cleanup_resources || true' EXIT

keeper_pipe_alive() {
  "$authority_python" - "$1" <<'PY' >/dev/null 2>&1
import ctypes
import os
import select
import sys
import time

descriptor = int(sys.argv[1])
deadline = time.monotonic() + 1.5

if os.name == "nt":
    import msvcrt
    from ctypes import wintypes

    peek_named_pipe = ctypes.windll.kernel32.PeekNamedPipe
    peek_named_pipe.argtypes = [
        wintypes.HANDLE,
        wintypes.LPVOID,
        wintypes.DWORD,
        wintypes.LPVOID,
        ctypes.POINTER(wintypes.DWORD),
        wintypes.LPVOID,
    ]
    peek_named_pipe.restype = wintypes.BOOL
    handle = wintypes.HANDLE(msvcrt.get_osfhandle(descriptor))

    def available_bytes():
        available = wintypes.DWORD()
        if not peek_named_pipe(
            handle, None, 0, None, ctypes.byref(available), None
        ):
            return None
        return available.value

    # Discard every heartbeat that existed before this liveness check. A
    # subsequently observed byte must therefore have been produced by the
    # exact worker that still owns the retained artifact handles.
    while True:
        available = available_bytes()
        if available is None:
            raise SystemExit(1)
        if available == 0:
            break
        if not os.read(descriptor, min(available, 65536)):
            raise SystemExit(1)
    while time.monotonic() < deadline:
        available = available_bytes()
        if available is None:
            raise SystemExit(1)
        if available:
            raise SystemExit(0 if os.read(descriptor, 1) else 1)
        time.sleep(0.02)
    raise SystemExit(1)

import fcntl

prior_flags = fcntl.fcntl(descriptor, fcntl.F_GETFL)
try:
    fcntl.fcntl(descriptor, fcntl.F_SETFL, prior_flags | os.O_NONBLOCK)
    while True:
        try:
            value = os.read(descriptor, 65536)
        except BlockingIOError:
            break
        if not value:
            raise SystemExit(1)
finally:
    fcntl.fcntl(descriptor, fcntl.F_SETFL, prior_flags)

ready, _, _ = select.select([descriptor], [], [], max(0.0, deadline - time.monotonic()))
if not ready:
    raise SystemExit(1)
raise SystemExit(0 if os.read(descriptor, 1) else 1)
PY
}

artifact_index() {
  local requested="$1"
  local index
  for ((index = 0; index < ${#artifact_names[@]}; index++)); do
    if [[ "${artifact_names[$index]}" == "$requested" ]]; then
      printf '%s' "$index"
      return 0
    fi
  done
  return 1
}

identity_for_descriptor() {
  "$authority_python" - "$1" <<'PY'
import ctypes
import os
import sys

fd = int(sys.argv[1])
if os.name != "nt":
    value = os.fstat(fd)
    print(f"{value.st_dev:016x} {value.st_ino:032x}")
    raise SystemExit(0)

import msvcrt
from ctypes import wintypes

class FILE_ID_128(ctypes.Structure):
    _fields_ = [("Low", ctypes.c_ulonglong), ("High", ctypes.c_ulonglong)]

class FILE_ID_INFO(ctypes.Structure):
    _fields_ = [("VolumeSerialNumber", ctypes.c_ulonglong), ("FileId", FILE_ID_128)]

info = FILE_ID_INFO()
handle = wintypes.HANDLE(msvcrt.get_osfhandle(fd))
get_info = ctypes.windll.kernel32.GetFileInformationByHandleEx
if not get_info(handle, 18, ctypes.byref(info), ctypes.sizeof(info)):
    raise ctypes.WinError()
print(
    f"{info.VolumeSerialNumber:016x}"
    f" {info.FileId.High:016x}{info.FileId.Low:016x}"
)
PY
}

start_artifact_keeper() {
  # The keeper creates each child with O_EXCL, opens and identity-checks a
  # read-only retained handle before releasing the creation writer, and holds
  # that creation identity through cleanup. Its descriptors are not inherited
  # by Docker, Cargo, or any restore helper launched by this shell.
  exec 7< <(
    "$authority_python" -c '
import ctypes
import os
import stat
import sys
import time

root = sys.argv[1]
names = sys.argv[2:]
if not names or len(names) != len(set(names)):
    raise RuntimeError("temporary artifact keeper manifest is invalid")

if os.name == "nt":
    import msvcrt
    from ctypes import wintypes

    class FILE_ID_128(ctypes.Structure):
        _fields_ = [("Low", ctypes.c_ulonglong), ("High", ctypes.c_ulonglong)]

    class FILE_ID_INFO(ctypes.Structure):
        _fields_ = [("VolumeSerialNumber", ctypes.c_ulonglong), ("FileId", FILE_ID_128)]

    get_info = ctypes.windll.kernel32.GetFileInformationByHandleEx
    get_info.argtypes = [wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD]
    get_info.restype = wintypes.BOOL

    def identity(fd):
        info = FILE_ID_INFO()
        handle = wintypes.HANDLE(msvcrt.get_osfhandle(fd))
        if not get_info(handle, 18, ctypes.byref(info), ctypes.sizeof(info)):
            raise ctypes.WinError()
        return (
            f"{info.VolumeSerialNumber:016x}",
            f"{info.FileId.High:016x}{info.FileId.Low:016x}",
        )
else:
    def identity(fd):
        value = os.fstat(fd)
        return f"{value.st_dev:016x}", f"{value.st_ino:032x}"

descriptors = []
try:
    for name in names:
        if not name or "/" in name or "\\" in name or name in (".", ".."):
            raise RuntimeError("temporary artifact keeper name is unsafe")
        flags = os.O_RDWR | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
        if os.name != "nt":
            flags |= os.O_NOFOLLOW | os.O_CLOEXEC
        path = os.path.join(root, name)
        descriptor = os.open(path, flags, 0o600)
        if os.name != "nt":
            os.fchmod(descriptor, 0o600)
        value = os.fstat(descriptor)
        if not stat.S_ISREG(value.st_mode) or value.st_nlink != 1:
            raise RuntimeError("temporary artifact keeper did not retain one regular file")
        os.fsync(descriptor)
        storage_id, file_id = identity(descriptor)
        retained_flags = os.O_RDONLY | getattr(os, "O_BINARY", 0)
        if os.name != "nt":
            retained_flags |= os.O_NOFOLLOW | os.O_CLOEXEC
        retained_descriptor = os.open(path, retained_flags)
        retained_value = os.fstat(retained_descriptor)
        if (
            identity(retained_descriptor) != (storage_id, file_id)
            or not stat.S_ISREG(retained_value.st_mode)
            or retained_value.st_nlink != 1
        ):
            os.close(retained_descriptor)
            os.close(descriptor)
            raise RuntimeError(
                "temporary artifact changed while retaining its creation identity"
            )
        # Holding writable descriptors would make generated shell wrappers
        # unexecutable on Linux (ETXTBSY). Retain the verified read-only handle
        # before releasing the O_EXCL creation writer.
        descriptors.append(retained_descriptor)
        os.close(descriptor)
        print(f"{name}|{storage_id}|{file_id}", flush=True)
    print("ready", flush=True)
    if os.environ.get("WORLDSTREAM_NATIVE_PG_TEST_ARTIFACT_KEEPER_EXIT_AFTER_READY") == "1":
        raise SystemExit(97)
    while True:
        try:
            os.write(1, b".")
        except BrokenPipeError:
            devnull = os.open(os.devnull, os.O_WRONLY)
            os.dup2(devnull, 1)
            os.close(devnull)
            break
        time.sleep(0.5)
finally:
    for descriptor in reversed(descriptors):
        os.close(descriptor)
' "$authority_root" "${artifact_plan[@]}" &
    artifact_keeper_worker_pid=$!
    wait "$artifact_keeper_worker_pid"
  )
  artifact_keeper_started=1

  local expected_name manifest_line name storage_id file_id
  for expected_name in "${artifact_plan[@]}"; do
    if ! IFS= read -r manifest_line <&7; then
      return 1
    fi
    IFS='|' read -r name storage_id file_id <<<"$manifest_line"
    if [[ "$name" != "$expected_name" \
        || ! "$storage_id" =~ ^[0-9a-f]{16}$ \
        || ! "$file_id" =~ ^[0-9a-f]{32}$ ]]; then
      return 1
    fi
    artifact_names+=("$name")
    artifact_kinds+=("file")
    artifact_storage_ids+=("$storage_id")
    artifact_file_ids+=("$file_id")
  done
  if ! read -r keeper_ready <&7 || [[ "$keeper_ready" != "ready" ]]; then
    return 1
  fi
  return 0
}

open_artifact_fd() {
  local name="$1"
  local output_variable="$2"
  local truncate="${3:-0}"
  local index opened_fd actual_storage_id actual_file_id
  index="$(artifact_index "$name")" || return 1
  opened_fd="$next_artifact_fd"
  next_artifact_fd=$((next_artifact_fd + 1))
  eval "exec ${opened_fd}<>\"\$temp_root/\$name\"" || return 1
  if ! read -r actual_storage_id actual_file_id < <(identity_for_descriptor "$opened_fd"); then
    eval "exec ${opened_fd}>&-"
    return 1
  fi
  if [[ "$actual_storage_id" != "${artifact_storage_ids[$index]}" \
      || "$actual_file_id" != "${artifact_file_ids[$index]}" ]]; then
    eval "exec ${opened_fd}>&-"
    return 1
  fi
  if [[ "$truncate" -eq 1 ]]; then
    if ! "$authority_python" -c 'import os,sys; fd=int(sys.argv[1]); os.ftruncate(fd,0); os.lseek(fd,0,0)' "$opened_fd"; then
      eval "exec ${opened_fd}>&-"
      return 1
    fi
  fi
  printf -v "$output_variable" '%s' "$opened_fd"
}

open_artifact_read_fd() {
  local name="$1"
  local output_variable="$2"
  local index opened_fd actual_storage_id actual_file_id
  index="$(artifact_index "$name")" || return 1
  opened_fd="$next_artifact_fd"
  next_artifact_fd=$((next_artifact_fd + 1))
  eval "exec ${opened_fd}<\"\$temp_root/\$name\"" || return 1
  if ! read -r actual_storage_id actual_file_id \
      < <(identity_for_descriptor "$opened_fd"); then
    eval "exec ${opened_fd}<&-"
    return 1
  fi
  if [[ "$actual_storage_id" != "${artifact_storage_ids[$index]}" \
      || "$actual_file_id" != "${artifact_file_ids[$index]}" ]]; then
    eval "exec ${opened_fd}<&-"
    return 1
  fi
  printf -v "$output_variable" '%s' "$opened_fd"
}

close_artifact_fd() {
  local descriptor="$1"
  eval "exec ${descriptor}>&-"
}

write_artifact() {
  local name="$1"
  local descriptor
  open_artifact_fd "$name" descriptor 1 || return 1
  if ! "$authority_python" -c '
import os,sys
fd=int(sys.argv[1]); data=sys.stdin.buffer.read(); view=memoryview(data)
while view:
    count=os.write(fd,view)
    if count <= 0: raise RuntimeError("temporary artifact write made no progress")
    view=view[count:]
os.fsync(fd)
' "$descriptor"; then
    close_artifact_fd "$descriptor"
    return 1
  fi
  close_artifact_fd "$descriptor"
}

read_artifact() {
  local name="$1"
  local descriptor
  open_artifact_fd "$name" descriptor 0 || return 1
  "$authority_python" -c '
import os,sys
fd=int(sys.argv[1]); os.lseek(fd,0,0)
while True:
    value=os.read(fd,65536)
    if not value: break
    sys.stdout.buffer.write(value)
' "$descriptor"
  local status=$?
  close_artifact_fd "$descriptor"
  return "$status"
}

set_artifact_mode() {
  local name="$1" mode="$2" descriptor
  open_artifact_fd "$name" descriptor 0 || return 1
  "$authority_python" -c 'import os,sys; os.fchmod(int(sys.argv[1]),int(sys.argv[2],8))' \
    "$descriptor" "$mode"
  local status=$?
  close_artifact_fd "$descriptor"
  return "$status"
}

retained_executable_artifact_matches() {
  local name="$1" descriptor="$2"
  local index actual_storage_id actual_file_id
  index="$(artifact_index "$name")" || return 1
  if ! "$authority_python" - "$descriptor" <<'PY' >/dev/null 2>&1
import os
import stat
import sys

value = os.fstat(int(sys.argv[1]))
raise SystemExit(
    not stat.S_ISREG(value.st_mode)
    or value.st_nlink != 1
    or value.st_size == 0
    or value.st_mode & 0o100 == 0
)
PY
  then
    return 1
  fi
  read -r actual_storage_id actual_file_id \
    < <(identity_for_descriptor "$descriptor") || return 1
  [[ "$actual_storage_id" == "${artifact_storage_ids[$index]}" \
      && "$actual_file_id" == "${artifact_file_ids[$index]}" ]]
}

retain_receipt_artifact() {
  local name="$1" storage_id="$2" file_id="$3" output_variable="$4"
  local descriptor actual_storage_id actual_file_id
  [[ "$name" != */* && "$name" != *\\* && "$name" != "." && "$name" != ".." ]] || return 1
  [[ "$storage_id" =~ ^[0-9a-f]{16}$ && "$file_id" =~ ^[0-9a-f]{32}$ ]] || return 1
  descriptor="$next_artifact_fd"
  next_artifact_fd=$((next_artifact_fd + 1))
  eval "exec ${descriptor}<>\"\$temp_root/\$name\"" || return 1
  if ! read -r actual_storage_id actual_file_id < <(identity_for_descriptor "$descriptor") \
      || [[ "$actual_storage_id" != "$storage_id" || "$actual_file_id" != "$file_id" ]]; then
    eval "exec ${descriptor}>&-"
    return 1
  fi
  artifact_names+=("$name")
  artifact_kinds+=("file")
  artifact_storage_ids+=("$storage_id")
  artifact_file_ids+=("$file_id")
  printf -v "$output_variable" '%s' "$descriptor"
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --evidence)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --evidence requires a path' >&2; exit "$EXIT_CONFIGURATION"; }
      evidence_file="$2"
      shift 2
      ;;
    --source-host)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --source-host requires a value' >&2; exit "$EXIT_CONFIGURATION"; }
      source_host="$2"
      shift 2
      ;;
    --source-port)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --source-port requires a value' >&2; exit "$EXIT_CONFIGURATION"; }
      source_port="$2"
      shift 2
      ;;
    --source-tls-mode)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --source-tls-mode requires a value' >&2; exit "$EXIT_CONFIGURATION"; }
      source_tls_mode="$2"
      shift 2
      ;;
    --target-host)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --target-host requires a value' >&2; exit "$EXIT_CONFIGURATION"; }
      target_host="$2"
      shift 2
      ;;
    --target-port)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --target-port requires a value' >&2; exit "$EXIT_CONFIGURATION"; }
      target_port="$2"
      shift 2
      ;;
    --target-tls-mode)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --target-tls-mode requires a value' >&2; exit "$EXIT_CONFIGURATION"; }
      target_tls_mode="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit "$EXIT_PASS"
      ;;
    *)
      printf '%s\n' 'native PostgreSQL restore: unknown argument' >&2
      exit "$EXIT_CONFIGURATION"
      ;;
  esac
done

if [[ -n "$evidence_file" && "$evidence_file" != /* ]]; then
  evidence_file="$invocation_dir/$evidence_file"
fi

resolve_tool() {
  local override="$1"
  local name="$2"
  if [[ -n "$override" ]]; then
    [[ -x "$override" ]] && printf '%s' "$override"
    return 0
  fi
  command -v "$name" 2>/dev/null || true
}

resolve_python() {
  local override="$1"
  local candidate=""
  local name=""
  if [[ -n "$override" ]]; then
    if [[ -x "$override" ]] \
      && "$override" -c 'import sys; raise SystemExit(sys.version_info[:3] != (3, 14, 7))' \
        >/dev/null 2>&1; then
      printf '%s' "$override"
    fi
    return 0
  fi
  for name in python3 python; do
    candidate="$(command -v "$name" 2>/dev/null || true)"
    if [[ -n "$candidate" && -x "$candidate" ]] \
      && "$candidate" -c 'import sys; raise SystemExit(sys.version_info[:3] != (3, 14, 7))' \
        >/dev/null 2>&1; then
      printf '%s' "$candidate"
      return 0
    fi
  done
  if command -v uv >/dev/null 2>&1; then
    candidate="$(uv run --python 3.14.7 --no-project python -c \
      'import sys; print(sys.executable)' 2>/dev/null || true)"
    if [[ -n "$candidate" && -x "$candidate" ]] \
      && "$candidate" -c 'import sys; raise SystemExit(sys.version_info[:3] != (3, 14, 7))' \
        >/dev/null 2>&1; then
      printf '%s' "$candidate"
    fi
  fi
}

docker_bin="$(resolve_tool "$docker_bin" docker)"
cargo_bin="$(resolve_tool "$cargo_bin" cargo)"
python_bin="$(resolve_python "$python_bin")"
pg_dump_bin="$(resolve_tool "$pg_dump_bin" pg_dump)"
pg_restore_bin="$(resolve_tool "$pg_restore_bin" pg_restore)"
psql_bin="$(resolve_tool "$psql_bin" psql)"

publish_evidence() {
  local payload="$1"
  [[ -n "$evidence_file" ]] || return 0
  "$python_bin" - "$evidence_file" "$payload" <<'PY'
import os
import stat
import subprocess
import sys

requested = os.path.abspath(sys.argv[1])
path = os.path.join(
    os.path.realpath(os.path.dirname(requested)), os.path.basename(requested)
)
payload = (sys.argv[2] + "\n").encode()
if os.name == "nt":
    import ctypes
    from ctypes import wintypes

    parent = os.path.dirname(path)
    create = ctypes.windll.kernel32.CreateFileW
    create.argtypes = [
        wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
        wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE,
    ]
    create.restype = wintypes.HANDLE
    parent_handle = create(
        parent,
        0x80000000,
        0x00000001 | 0x00000002,
        None,
        3,
        0x02000000 | 0x00200000,
        None,
    )
    if parent_handle == wintypes.HANDLE(-1).value:
        raise ctypes.WinError()
    try:
        parent_before = os.stat(parent, follow_symlinks=False)
        hook = os.environ.get("WORLDSTREAM_NATIVE_PG_TEST_EVIDENCE_PARENT_HOOK")
        if hook:
            subprocess.run([hook, parent], check=True)
        create_flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
        output_fd = os.open(path, create_flags, 0o600)
        try:
            before = os.fstat(output_fd)
            if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
                raise RuntimeError("evidence output is not one regular file")
            view = memoryview(payload)
            while view:
                written = os.write(output_fd, view)
                if written <= 0:
                    raise RuntimeError("evidence output made no progress")
                view = view[written:]
            os.fsync(output_fd)
            after = os.fstat(output_fd)
            named = os.stat(path, follow_symlinks=False)
            if (
                (after.st_dev, after.st_ino) != (before.st_dev, before.st_ino)
                or (named.st_dev, named.st_ino) != (before.st_dev, before.st_ino)
                or after.st_nlink != 1
                or after.st_size != len(payload)
            ):
                raise RuntimeError("evidence output identity changed")
        finally:
            os.close(output_fd)
        parent_after = os.stat(parent, follow_symlinks=False)
        if (parent_after.st_dev, parent_after.st_ino) != (
            parent_before.st_dev,
            parent_before.st_ino,
        ):
            raise RuntimeError("evidence parent identity changed")
    finally:
        ctypes.windll.kernel32.CloseHandle(parent_handle)
    raise SystemExit(0)

parts = [part for part in path.split(os.sep) if part]
if not parts:
    raise RuntimeError("evidence path has no filename")

flags = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC
if hasattr(os, "O_NOFOLLOW"):
    flags |= os.O_NOFOLLOW
directory_fds = [os.open(os.sep, flags)]
try:
    for component in parts[:-1]:
        if component in (".", ".."):
            raise RuntimeError("evidence path traversal is forbidden")
        directory_fds.append(os.open(component, flags, dir_fd=directory_fds[-1]))
    parent_fd = directory_fds[-1]
    parent_before = os.fstat(parent_fd)
    hook = os.environ.get("WORLDSTREAM_NATIVE_PG_TEST_EVIDENCE_PARENT_HOOK")
    if hook:
        subprocess.run([hook, os.path.dirname(path)], check=True)
    create_flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        create_flags |= os.O_NOFOLLOW
    output_fd = os.open(parts[-1], create_flags, 0o600, dir_fd=parent_fd)
    try:
        before = os.fstat(output_fd)
        if (
            not stat.S_ISREG(before.st_mode)
            or before.st_nlink != 1
            or before.st_uid != os.geteuid()
        ):
            raise RuntimeError("evidence output is not one owner-only file")
        view = memoryview(payload)
        while view:
            written = os.write(output_fd, view)
            if written <= 0:
                raise RuntimeError("evidence output made no progress")
            view = view[written:]
        os.fsync(output_fd)
        after = os.fstat(output_fd)
        named = os.stat(parts[-1], dir_fd=parent_fd, follow_symlinks=False)
        if (
            (after.st_dev, after.st_ino) != (before.st_dev, before.st_ino)
            or (named.st_dev, named.st_ino) != (before.st_dev, before.st_ino)
            or after.st_nlink != 1
            or after.st_size != len(payload)
        ):
            raise RuntimeError("evidence output identity changed")
        os.fsync(parent_fd)
        parent_after = os.fstat(parent_fd)
        if (parent_after.st_dev, parent_after.st_ino) != (
            parent_before.st_dev,
            parent_before.st_ino,
        ):
            raise RuntimeError("evidence parent identity changed")
    finally:
        os.close(output_fd)
finally:
    for descriptor in reversed(directory_fds):
        os.close(descriptor)
PY
}

write_static() {
  local status="$1" reason="$2" code="$3"
  local json
  json="$("$python_bin" - "$status" "$reason" "$code" <<'PY'
import json, sys
print(json.dumps({
    "schema": "worldstream/native-postgres-restore-smoke-evidence/v1",
    "status": sys.argv[1],
    "reason": sys.argv[2],
    "exit_code": int(sys.argv[3]),
    "release_evidence": False,
    "native_restore": None,
    "live_restore_concurrency": None,
    "target_isolated": False,
    "target_published": False,
    "secrets_emitted": False,
}, sort_keys=True, separators=(",", ":")))
PY
  )"
  if [[ -n "$evidence_file" ]]; then
    publish_evidence "$json"
  fi
  printf '%s\n' "$json"
}

if [[ -z "$python_bin" || ! -x "$python_bin" ]]; then
  printf '%s\n' '{"exit_code":10,"live_restore_concurrency":null,"native_restore":null,"reason":"pinned_python_unavailable","release_evidence":false,"schema":"worldstream/native-postgres-restore-smoke-evidence/v1","secrets_emitted":false,"status":"unavailable","target_isolated":false,"target_published":false}'
  exit "$EXIT_UNAVAILABLE"
fi

if [[ -z "$cargo_bin" || ! -x "$cargo_bin" ]]; then
  write_static unavailable cargo_bin_unavailable "$EXIT_UNAVAILABLE"
  exit "$EXIT_UNAVAILABLE"
fi

if [[ -n "$source_host" && -n "$source_port" && -n "$target_host" && -n "$target_port" ]]; then
  for required in pg_dump_bin pg_restore_bin psql_bin; do
    if [[ -z "${!required}" || ! -x "${!required}" ]]; then
      write_static unavailable "${required}_unavailable" "$EXIT_UNAVAILABLE"
      exit "$EXIT_UNAVAILABLE"
    fi
  done
fi

# All shell-owned temporary children are created once with O_EXCL by a keeper
# that retains verified read-only handles to their creation identities through
# cleanup. Rust publications are added later from the retained-handle receipt
# returned by worldstreamctl; no expected child is first admitted during cleanup.
if ! start_artifact_keeper; then
  write_static incomplete native_temp_artifact_creation_failed "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi
artifact_root="$temp_root_path"
mount_authority_token="$($authority_python -c 'import secrets; print(secrets.token_hex(32))')"
printf '%s\n' "$mount_authority_token" | write_artifact mount-authority

if [[ -z "$source_host" || -z "$source_port" || -z "$target_host" || -z "$target_port" ]]; then
  if [[ -z "$docker_bin" || ! -x "$docker_bin" ]] || ! "$docker_bin" info >/dev/null 2>&1; then
    write_static unavailable docker_unavailable "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi
  source_container="worldstream-native-pg-source-$RANDOM"
  target_container="worldstream-native-pg-target-$RANDOM"
  password="worldstream_native_pg_$(date +%s)_$RANDOM"
  if ! retained_cwd_matches; then
    write_static incomplete native_temp_root_identity_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  if [[ "${OS:-}" == "Windows_NT" ]]; then
    write_static unavailable native_docker_smoke_windows_uses_hosted_powershell_lane "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi
  if [[ "$(uname -s)" != "Linux" ]]; then
    write_static unavailable native_docker_retained_mount_authority_unavailable "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi
  docker_mount_root="/proc/$$/fd/$temp_root_fd"
  if [[ -n "${WORLDSTREAM_NATIVE_PG_TEST_BEFORE_DOCKER_MOUNT:-}" ]]; then
    "${WORLDSTREAM_NATIVE_PG_TEST_BEFORE_DOCKER_MOUNT}" "$temp_root_path"
  fi
  if ! retained_cwd_matches; then
    write_static incomplete native_temp_root_identity_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  if ! read -r named_root_storage_id named_root_file_id < <(identity_for_path "$authority_root") \
      || [[ "$named_root_storage_id" != "$temp_root_storage_id" \
        || "$named_root_file_id" != "$temp_root_file_id" ]]; then
    write_static incomplete native_docker_mount_source_identity_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  docker_env_file="$artifact_root/docker.env"
  passfile="$artifact_root/pgpass"
  umask 077
  printf 'POSTGRES_PASSWORD=%s\nPOSTGRES_DB=%s\n' "$password" "$source_db" | write_artifact docker.env
  printf '*:*:%s:%s:%s\n*:*:%s:%s:%s\n' \
    "$source_db" "$source_user" "$password" \
    "$target_db" "$target_user" "$password" | write_artifact pgpass
  open_artifact_fd docker.env docker_env_fd 0
  open_artifact_fd pgpass docker_passfile_fd 0
  open_artifact_fd restore-input.dump restore_input_fd 1
  open_artifact_fd restore-concurrency.observation restore_observation_fd 1
  docker_env_mount="/proc/$$/fd/$docker_env_fd"
  docker_passfile_mount="/proc/$$/fd/$docker_passfile_fd"
  docker_restore_input_mount="/proc/$$/fd/$restore_input_fd"
  if ! source_container_id="$("$docker_bin" run --detach --rm \
    --name "$source_container" -p 127.0.0.1::5432 \
    --env-file "$docker_env_mount" \
    --mount "type=bind,source=$docker_passfile_mount,target=/run/secrets/worldstream-pgpass,readonly" \
    --mount "type=bind,source=$docker_mount_root,target=/run/worldstream-native-pg,readonly" \
    --mount "type=bind,source=$docker_restore_input_mount,target=/run/worldstream-native-pg/restore-input.dump,readonly" \
    postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73)"; then
    close_artifact_fd "$docker_env_fd"
    close_artifact_fd "$docker_passfile_fd"
    write_static unavailable docker_container_start_failed "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi
  [[ "$source_container_id" =~ ^[0-9a-f]{64}$ ]] || {
    write_static incomplete docker_source_container_identity_invalid "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  }
  owned_docker=1
  if ! target_container_id="$("$docker_bin" run --detach --rm \
    --name "$target_container" -p 127.0.0.1::5432 \
    --env-file "$docker_env_mount" \
    --mount "type=bind,source=$docker_passfile_mount,target=/run/secrets/worldstream-pgpass,readonly" \
    --mount "type=bind,source=$docker_mount_root,target=/run/worldstream-native-pg,readonly" \
    --mount "type=bind,source=$docker_restore_input_mount,target=/run/worldstream-native-pg/restore-input.dump,readonly" \
    postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73)"; then
    close_artifact_fd "$docker_env_fd"
    close_artifact_fd "$docker_passfile_fd"
    write_static unavailable docker_container_start_failed "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi
  close_artifact_fd "$docker_env_fd"
  close_artifact_fd "$docker_passfile_fd"
  [[ "$target_container_id" =~ ^[0-9a-f]{64}$ ]] || {
    write_static incomplete docker_target_container_identity_invalid "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  }
  for container in "$source_container" "$target_container"; do
    mounted_authority_token="$("$docker_bin" exec "$container" \
      /bin/cat /run/worldstream-native-pg/mount-authority 2>/dev/null || true)"
    if [[ "$mounted_authority_token" != "$mount_authority_token" ]]; then
      write_static incomplete native_docker_mount_authority_binding_failed "$EXIT_INCOMPLETE"
      exit "$EXIT_INCOMPLETE"
    fi
  done
  source_port="$($docker_bin port "$source_container" 5432/tcp | sed -n 's/.*:\([0-9][0-9]*\)$/\1/p')"
  target_port="$($docker_bin port "$target_container" 5432/tcp | sed -n 's/.*:\([0-9][0-9]*\)$/\1/p')"
  source_host="127.0.0.1"
  target_host="127.0.0.1"
  source_tls_mode="disable"
  target_tls_mode="disable"
  for container in "$source_container" "$target_container"; do
    for _ in $(seq 1 60); do
      if "$docker_bin" exec --env PGPASSFILE=/run/secrets/worldstream-pgpass "$container" \
        psql --no-password --host 127.0.0.1 --port 5432 --dbname "$source_db" \
        --username "$source_user" --quiet --command 'SELECT 1' >/dev/null 2>&1; then break; fi
      sleep 1
    done
  done
  target_marker_sql="DO \$worldstream\$ BEGIN EXECUTE format('COMMENT ON DATABASE %I IS %L', current_database(), 'worldstream/native-postgres-disposable-target/v1'); END \$worldstream\$;"
  open_artifact_fd target-marker.stdout target_marker_stdout_fd 1
  open_artifact_fd target-marker.stderr target_marker_stderr_fd 1
  if ! "$docker_bin" exec --env PGPASSFILE=/run/secrets/worldstream-pgpass "$target_container" \
    psql --no-password --host 127.0.0.1 --port 5432 --dbname "$target_db" \
    --username "$target_user" --quiet --set ON_ERROR_STOP=1 --command "$target_marker_sql" \
    >&"$target_marker_stdout_fd" 2>&"$target_marker_stderr_fd"; then
    close_artifact_fd "$target_marker_stdout_fd"
    close_artifact_fd "$target_marker_stderr_fd"
    write_static incomplete native_target_marker_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  close_artifact_fd "$target_marker_stdout_fd"
  close_artifact_fd "$target_marker_stderr_fd"
  runtime_role="worldstream_native_runtime"
  runtime_password="${password}_runtime"
  transfer_abort_db="worldstream_transfer_abort_probe"
  runtime_setup_sql="CREATE ROLE $runtime_role LOGIN PASSWORD '$runtime_password' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION; ALTER DEFAULT PRIVILEGES FOR ROLE $source_user IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO $runtime_role; ALTER DEFAULT PRIVILEGES FOR ROLE $source_user IN SCHEMA public GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO $runtime_role; GRANT USAGE ON SCHEMA public TO $runtime_role;"
  open_artifact_fd runtime-role.stdout runtime_role_stdout_fd 1
  open_artifact_fd runtime-role.stderr runtime_role_stderr_fd 1
  if ! "$docker_bin" exec --env PGPASSFILE=/run/secrets/worldstream-pgpass "$source_container" \
    psql --no-password --host 127.0.0.1 --port 5432 --dbname "$source_db" \
    --username "$source_user" --quiet --set ON_ERROR_STOP=1 --command "$runtime_setup_sql" \
    >&"$runtime_role_stdout_fd" 2>&"$runtime_role_stderr_fd"; then
    close_artifact_fd "$runtime_role_stdout_fd"
    close_artifact_fd "$runtime_role_stderr_fd"
    write_static incomplete native_source_runtime_role_setup_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  close_artifact_fd "$runtime_role_stdout_fd"
  close_artifact_fd "$runtime_role_stderr_fd"
  open_artifact_fd transfer-abort-database.stdout transfer_abort_stdout_fd 1
  open_artifact_fd transfer-abort-database.stderr transfer_abort_stderr_fd 1
  if ! "$docker_bin" exec --env PGPASSFILE=/run/secrets/worldstream-pgpass "$source_container" \
    createdb --host 127.0.0.1 --port 5432 --username "$source_user" \
      "$transfer_abort_db" \
    >&"$transfer_abort_stdout_fd" 2>&"$transfer_abort_stderr_fd"; then
    close_artifact_fd "$transfer_abort_stdout_fd"
    close_artifact_fd "$transfer_abort_stderr_fd"
    write_static incomplete native_source_abort_database_setup_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  close_artifact_fd "$transfer_abort_stdout_fd"
  close_artifact_fd "$transfer_abort_stderr_fd"
  printf '%s' "host=$source_host port=$source_port user=$source_user password=$password dbname=$source_db" \
    | write_artifact transfer-admin.dsn
  printf '%s' "host=$source_host port=$source_port user=$runtime_role password=$runtime_password dbname=$source_db" \
    | write_artifact transfer-runtime.dsn
  printf '%s' "host=$source_host port=$source_port user=$source_user password=$password dbname=$transfer_abort_db" \
    | write_artifact transfer-abort-admin.dsn
  open_artifact_fd source-seed.stdout seed_stdout_fd 1
  open_artifact_fd source-seed.stderr seed_stderr_fd 1
  set +e
    WORLDSTREAM_PG_TRANSFER_MODE=external \
    WORLDSTREAM_PG_TRANSFER_BUILD_SOURCE=1 \
    WORLDSTREAM_PG_TRANSFER_CARGO="$cargo_bin" \
    WORLDSTREAM_PG_TRANSFER_PYTHON="$python_bin" \
    WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE="$artifact_root/transfer-admin.dsn" \
    WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE="$artifact_root/transfer-runtime.dsn" \
    WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE="$artifact_root/transfer-abort-admin.dsn" \
    WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE="$runtime_role" \
    "$transfer_smoke_bin" --build-source >&"$seed_stdout_fd" 2>&"$seed_stderr_fd"
  seed_code=$?
  set -e
  if [[ "$seed_code" -ne 0 ]]; then
    if [[ "${WORLDSTREAM_NATIVE_PG_DEBUG:-0}" == "1" ]]; then
      read_artifact source-seed.stderr | sed -n '1,20p' >&2
    fi
    close_artifact_fd "$seed_stdout_fd"
    close_artifact_fd "$seed_stderr_fd"
    write_static incomplete native_source_seed_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  close_artifact_fd "$seed_stdout_fd"
  close_artifact_fd "$seed_stderr_fd"
  open_artifact_fd source-prepare.stdout prepare_stdout_fd 1
  open_artifact_fd source-prepare.stderr prepare_stderr_fd 1
  set +e
  "$cargo_bin" run --quiet --locked --manifest-path "$root_dir/crates/worldstream-postgres/Cargo.toml" \
    --bin postgres-native-prepare -- "$source_host" "$source_port" "$source_db" "$source_user" \
    "$source_tls_mode" "$passfile" \
    >&"$prepare_stdout_fd" 2>&"$prepare_stderr_fd"
  prepare_code=$?
  set -e
  if [[ "$prepare_code" -ne 0 ]]; then
    if [[ "${WORLDSTREAM_NATIVE_PG_DEBUG:-0}" == "1" ]]; then
      read_artifact source-prepare.stderr | sed -n '1,20p' >&2
    fi
    close_artifact_fd "$prepare_stdout_fd"
    close_artifact_fd "$prepare_stderr_fd"
    write_static incomplete native_source_snapshot_prepare_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  close_artifact_fd "$prepare_stdout_fd"
  close_artifact_fd "$prepare_stderr_fd"
fi

if [[ -z "$passfile" || -L "$passfile" || ! -f "$passfile" ]]; then
  write_static incomplete pgpassfile_not_a_regular_file "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi
if [[ ! "$source_port" =~ ^[1-9][0-9]{0,4}$ || ! "$target_port" =~ ^[1-9][0-9]{0,4}$ ]]; then
  write_static incomplete invalid_endpoint_port "$EXIT_CONFIGURATION"
  exit "$EXIT_CONFIGURATION"
fi
if [[ ! "$source_tls_mode" =~ ^(require|disable)$ || ! "$target_tls_mode" =~ ^(require|disable)$ ]]; then
  write_static incomplete invalid_endpoint_tls_mode "$EXIT_CONFIGURATION"
  exit "$EXIT_CONFIGURATION"
fi

dump_path="$artifact_root/source.dump"
report_path="$artifact_root/native-restore-report.json"
artifact_directory_storage_id="$temp_root_storage_id"
artifact_directory_file_id="$temp_root_file_id"
if [[ "$owned_docker" -eq 1 ]]; then
  make_docker_tool_wrapper() {
    local wrapper_name="$1"
    local container="$2"
    local tool="$3"
    write_artifact "$wrapper_name" <<EOF
#!/bin/bash
set -euo pipefail
args=()
while [[ "\$#" -gt 0 ]]; do
  case "\$1" in
    --host|--port)
      shift 2
      ;;
    "$artifact_root"/*)
      args+=("/run/worldstream-native-pg/\${1#"$artifact_root/"}")
      shift
      ;;
    *)
      args+=("\$1")
      shift
      ;;
  esac
done
container_passfile=/run/secrets/worldstream-pgpass
if [[ "\${PGPASSFILE:-}" == "$artifact_root"/* ]]; then
  container_passfile="/run/worldstream-native-pg/\${PGPASSFILE#"$artifact_root/"}"
fi
exec "$docker_bin" exec --env PGPASSFILE="\$container_passfile" \\
  --interactive \\
  --env PGSSLMODE="\${PGSSLMODE:?}" \\
  --env PGGSSENCMODE="\${PGGSSENCMODE:?}" \\
  "$container" "$tool" --host 127.0.0.1 --port 5432 "\${args[@]}"
EOF
    set_artifact_mode "$wrapper_name" 700
  }
  make_docker_tool_wrapper pg_dump "$source_container" pg_dump
  make_docker_tool_wrapper pg_restore "$target_container" pg_restore
  "$python_bin" - "$docker_bin" "$target_container" "$target_user" "$target_db" \
    "$python_bin" "$restore_input_fd" "$restore_observation_fd" <<'PY' | write_artifact pg_restore
import sys

(
    docker_bin,
    target_container,
    target_user,
    target_db,
    python_bin,
    restore_input_fd,
    observation_fd,
) = sys.argv[1:]
template = r'''#!/bin/bash
set -euo pipefail
readonly restore_input_fd=@RESTORE_INPUT_FD@
readonly observation_fd=@OBSERVATION_FD@
args=()
restore_role=""
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --host|--port)
      shift 2
      ;;
    --username)
      [[ "$#" -ge 2 ]] || exit 91
      restore_role="$2"
      args+=("$1" "$2")
      shift 2
      ;;
    *)
      args+=("$1")
      shift
      ;;
  esac
done
[[ "$restore_role" =~ ^worldstream_restore_[0-9a-f]{32}$ ]] || exit 92
"@PYTHON@" -c 'import os,sys; fd=int(sys.argv[1]); os.ftruncate(fd,0); os.lseek(fd,0,0)' "$restore_input_fd"
/bin/cat >&"$restore_input_fd"
"@PYTHON@" -c 'import os,sys; os.fsync(int(sys.argv[1]))' "$restore_input_fd"

admin_sql() {
  local database="$1" sql="$2"
  "@DOCKER@" exec --env PGPASSFILE=/run/secrets/worldstream-pgpass \
    "@TARGET_CONTAINER@" psql --no-password --host 127.0.0.1 --port 5432 \
    --dbname "$database" --username "@TARGET_USER@" --quiet --tuples-only \
    --no-align --set ON_ERROR_STOP=1 --command "$sql"
}

probe_installed=0
restore_cli_pid=""
cleanup_probe() {
  local prior_status=$?
  set +e
  if [[ "$probe_installed" -eq 1 ]]; then
    admin_sql "@TARGET_DB@" 'UPDATE worldstream_smoke_control.restore_gate SET released=true' >/dev/null 2>&1
  fi
  [[ -z "$restore_cli_pid" ]] || wait "$restore_cli_pid" >/dev/null 2>&1
  if [[ "$probe_installed" -eq 1 ]]; then
    admin_sql "@TARGET_DB@" 'DROP EVENT TRIGGER IF EXISTS worldstream_smoke_restore_ddl_hold; DROP SCHEMA IF EXISTS worldstream_smoke_control CASCADE' >/dev/null 2>&1
  fi
  return "$prior_status"
}
trap cleanup_probe EXIT HUP INT TERM

setup_sql="CREATE SCHEMA worldstream_smoke_control; CREATE TABLE worldstream_smoke_control.restore_gate(released boolean NOT NULL); INSERT INTO worldstream_smoke_control.restore_gate VALUES(false); CREATE FUNCTION worldstream_smoke_control.restore_ddl_hold() RETURNS event_trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS 'BEGIN IF session_user LIKE ''worldstream_restore_%'' THEN WHILE NOT (SELECT released FROM worldstream_smoke_control.restore_gate) LOOP PERFORM pg_catalog.pg_sleep(0.05); END LOOP; END IF; END'; CREATE EVENT TRIGGER worldstream_smoke_restore_ddl_hold ON ddl_command_start EXECUTE FUNCTION worldstream_smoke_control.restore_ddl_hold()"
admin_sql "@TARGET_DB@" "$setup_sql" >/dev/null
probe_installed=1

container_restore='set -eu
credential="$(mktemp /tmp/worldstream-restore-role.XXXXXX)"
chmod 600 "$credential"
exec 9<>"$credential"
rm -f "$credential"
/bin/cat >&9
tool="$(command -v pg_restore)"
case "$tool" in /usr/local/bin/pg_restore|/usr/bin/pg_restore) ;; *) exit 95;; esac
exec 8<"$tool"
before="$(sha256sum /proc/self/fd/8 | cut -d" " -f1)"
PGPASSFILE=/proc/self/fd/9 /proc/self/fd/8 --host 127.0.0.1 --port 5432 "$@" /run/worldstream-native-pg/restore-input.dump
status=$?
after="$(sha256sum /proc/self/fd/8 | cut -d" " -f1)"
exec 9>&-
test "$before" = "$after"
exit "$status"'
/bin/cat "${PGPASSFILE:?}" | "@DOCKER@" exec --interactive \
  --env PGSSLMODE="${PGSSLMODE:?}" --env PGGSSENCMODE="${PGGSSENCMODE:?}" \
  "@TARGET_CONTAINER@" /bin/sh -c "$container_restore" worldstream-pg-restore "${args[@]}" &
restore_cli_pid=$!

observation=""
for _ in $(seq 1 120); do
  observation="$(admin_sql postgres "WITH db AS (SELECT oid,datconnlimit FROM pg_database WHERE datname=@TARGET_DB_SQL_LITERAL@), sessions AS (SELECT a.pid,a.usename,r.rolsuper FROM pg_stat_activity a JOIN pg_roles r ON r.rolname=a.usename,db WHERE a.datid=db.oid AND a.pid<>pg_backend_pid() AND a.backend_type='client backend'), role_state AS (SELECT rolcanlogin,rolsuper,rolconnlimit FROM pg_roles WHERE rolname='$restore_role') SELECT db.datconnlimit::text || '|' || count(s.pid)::text || '|' || count(*) FILTER (WHERE s.rolsuper)::text || '|' || count(*) FILTER (WHERE s.usename='$restore_role')::text || '|' || COALESCE((SELECT rolcanlogin::text FROM role_state),'') || '|' || COALESCE((SELECT rolsuper::text FROM role_state),'') || '|' || COALESCE((SELECT rolconnlimit::text FROM role_state),'') || '|' || COALESCE(min(s.pid) FILTER (WHERE s.rolsuper)::text,'') || '|' || COALESCE(min(s.pid) FILTER (WHERE s.usename='$restore_role')::text,'') FROM db LEFT JOIN sessions s ON true GROUP BY db.datconnlimit")"
  IFS='|' read -r observed_limit observed_total observed_super observed_restore observed_login observed_role_super observed_role_limit keeper_pid restore_pid <<<"$observation"
  if [[ "$observed_limit" == "2" && "$observed_total" == "2" \
      && "$observed_super" == "1" && "$observed_restore" == "1" \
      && "$observed_login" == "true" && "$observed_role_super" == "false" \
      && "$observed_role_limit" == "1" && "$keeper_pid" =~ ^[1-9][0-9]*$ \
      && "$restore_pid" =~ ^[1-9][0-9]*$ && "$keeper_pid" != "$restore_pid" ]]; then
    break
  fi
  observation=""
  sleep 0.25
done
[[ -n "$observation" ]] || exit 94
printf '%s|%s\n' "$restore_role" "$observation" >&"$observation_fd"
"@PYTHON@" -c 'import os,sys; os.fsync(int(sys.argv[1]))' "$observation_fd"
admin_sql "@TARGET_DB@" 'UPDATE worldstream_smoke_control.restore_gate SET released=true' >/dev/null
wait "$restore_cli_pid"
restore_status=$?
restore_cli_pid=""
admin_sql "@TARGET_DB@" 'DROP EVENT TRIGGER worldstream_smoke_restore_ddl_hold; DROP SCHEMA worldstream_smoke_control CASCADE' >/dev/null
probe_installed=0
trap - EXIT HUP INT TERM
exit "$restore_status"
'''
for marker, value in {
    "@DOCKER@": docker_bin,
    "@TARGET_CONTAINER@": target_container,
    "@TARGET_USER@": target_user,
    "@TARGET_DB@": target_db,
    "@TARGET_DB_SQL_LITERAL@": "'" + target_db.replace("'", "''") + "'",
    "@PYTHON@": python_bin,
    "@RESTORE_INPUT_FD@": restore_input_fd,
    "@OBSERVATION_FD@": observation_fd,
}.items():
    template = template.replace(marker, value)
sys.stdout.write(template)
PY
  set_artifact_mode pg_restore 700
  write_artifact psql <<EOF
#!/bin/bash
set -euo pipefail
args=()
selected_container="$source_container"
selected_port=""
while [[ "\$#" -gt 0 ]]; do
  case "\$1" in
    --host)
      shift 2
      ;;
    --port)
      selected_port="\$2"
      shift 2
      ;;
    "$artifact_root"/*)
      args+=("/run/worldstream-native-pg/\${1#"$artifact_root/"}")
      shift
      ;;
    *)
      args+=("\$1")
      shift
      ;;
  esac
done
if [[ "\$selected_port" == "$target_port" ]]; then selected_container="$target_container"; fi
exec "$docker_bin" exec --env PGPASSFILE=/run/secrets/worldstream-pgpass \\
  --env PGSSLMODE="\${PGSSLMODE:?}" \\
  "\$selected_container" psql --host 127.0.0.1 --port 5432 "\${args[@]}"
EOF
  set_artifact_mode psql 700
  if ! open_artifact_read_fd pg_dump owned_pg_dump_fd \
      || ! open_artifact_read_fd pg_restore owned_pg_restore_fd \
      || ! open_artifact_read_fd psql owned_psql_fd; then
    write_static incomplete native_docker_tool_authority_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  # The standalone Docker lane is Linux-only. Give worldstreamctl executable
  # paths that resolve through descriptors retained from the artifact keeper's
  # creation identities; a later pathname replacement cannot become a tool.
  pg_dump_bin="/proc/$$/fd/$owned_pg_dump_fd"
  pg_restore_bin="/proc/$$/fd/$owned_pg_restore_fd"
  psql_bin="/proc/$$/fd/$owned_psql_fd"
  if [[ -n "${WORLDSTREAM_NATIVE_PG_TEST_BEFORE_DRIVER:-}" ]]; then
    if ! "${WORLDSTREAM_NATIVE_PG_TEST_BEFORE_DRIVER}" "$temp_root_path"; then
      write_static incomplete native_docker_tool_pre_driver_hook_failed "$EXIT_INCOMPLETE"
      exit "$EXIT_INCOMPLETE"
    fi
  fi
  if ! retained_executable_artifact_matches pg_dump "$owned_pg_dump_fd" \
      || ! retained_executable_artifact_matches pg_restore "$owned_pg_restore_fd" \
      || ! retained_executable_artifact_matches psql "$owned_psql_fd"; then
    write_static incomplete native_docker_tool_authority_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
fi
set +e
open_artifact_fd driver.stderr driver_stderr_fd 1
driver_output="$($cargo_bin run --quiet --locked --manifest-path "$root_dir/Cargo.toml" \
  -p worldstream-server --bin worldstreamctl -- postgres native restore \
  --source-host "$source_host" --source-port "$source_port" \
  --source-database "$source_db" --source-username "$source_user" \
  --source-tls-mode "$source_tls_mode" \
  --target-host "$target_host" --target-port "$target_port" \
  --target-database "$target_db" --target-username "$target_user" \
  --target-tls-mode "$target_tls_mode" --passfile "$passfile" \
  --pg-dump "$pg_dump_bin" --pg-restore "$pg_restore_bin" --psql "$psql_bin" \
  --dump "$dump_path" --report "$report_path" \
  --artifact-directory-storage-id "$artifact_directory_storage_id" \
  --artifact-directory-file-id "$artifact_directory_file_id" \
  --timeout-seconds 1200 2>&"$driver_stderr_fd")"
driver_code=$?
set -e
close_artifact_fd "$driver_stderr_fd"

if [[ "$driver_code" -ne 0 || -z "$driver_output" ]]; then
  if [[ "${WORLDSTREAM_NATIVE_PG_DEBUG:-0}" == "1" ]]; then
    printf 'native_driver_code=%s\n' "$driver_code" >&2
    read_artifact driver.stderr | sed -n '1,20p' >&2
  fi
  write_static incomplete native_driver_failed "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi

receipt_line="$(printf '%s\n' "$driver_output" | tail -n 1)"
receipt_fields="$($python_bin - "$receipt_line" <<'PY'
import json
import re
import sys

receipt = json.loads(sys.argv[1])
expected_fields = {
    "schema",
    "status",
    "report_digest",
    "report_size_bytes",
    "native_dump_digest",
    "native_dump_size_bytes",
    "native_dump_identity",
    "report_identity",
    "recovery_record_identity",
    "recovery_record_name",
    "source_provider_identity",
    "target_provider_identity",
    "secrets_emitted",
}
if (
    set(receipt) != expected_fields
    or receipt.get("schema") != "worldstream/postgres-native-restore-receipt/v1"
    or receipt.get("status") != "committed"
    or not isinstance(receipt.get("report_digest"), str)
    or re.fullmatch(r"blake3:[0-9a-f]{64}", receipt["report_digest"]) is None
    or type(receipt.get("report_size_bytes")) is not int
    or receipt["report_size_bytes"] <= 0
    or not isinstance(receipt.get("native_dump_digest"), str)
    or re.fullmatch(r"[0-9a-f]{64}", receipt["native_dump_digest"]) is None
    or type(receipt.get("native_dump_size_bytes")) is not int
    or receipt["native_dump_size_bytes"] <= 0
    or receipt.get("secrets_emitted") is not False
):
    raise RuntimeError("native restore receipt is not committed")
identities = [
    receipt.get("native_dump_identity"),
    receipt.get("report_identity"),
    receipt.get("recovery_record_identity"),
]
for identity in identities:
    if (
        not isinstance(identity, dict)
        or set(identity) != {"storage_id", "file_id"}
        or re.fullmatch(r"[0-9a-f]{16}", identity["storage_id"]) is None
        or re.fullmatch(r"[0-9a-f]{32}", identity["file_id"]) is None
    ):
        raise RuntimeError("native restore receipt identity is malformed")
name = receipt.get("recovery_record_name")
if not isinstance(name, str) or re.fullmatch(
    r"[.]worldstream_native_recovery_[0-9a-f]{32}[.]json", name
) is None:
    raise RuntimeError("native restore recovery record name is malformed")

providers = [
    receipt.get("source_provider_identity"),
    receipt.get("target_provider_identity"),
]
for provider in providers:
    if (
        not isinstance(provider, dict)
        or set(provider) != {"system_identifier", "database_oid", "database_name"}
        or not isinstance(provider["system_identifier"], str)
        or not provider["system_identifier"].isdigit()
        or int(provider["system_identifier"]) <= 0
        or not isinstance(provider["database_oid"], str)
        or not provider["database_oid"].isdigit()
        or int(provider["database_oid"]) <= 0
        or not isinstance(provider["database_name"], str)
        or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,62}", provider["database_name"])
        is None
    ):
        raise RuntimeError("native restore provider identity is malformed")
print(
    identities[0]["storage_id"], identities[0]["file_id"],
    identities[1]["storage_id"], identities[1]["file_id"],
    identities[2]["storage_id"], identities[2]["file_id"], name,
    receipt["report_digest"], receipt["report_size_bytes"],
    receipt["native_dump_digest"], receipt["native_dump_size_bytes"],
    providers[0]["system_identifier"], providers[0]["database_oid"],
    providers[0]["database_name"], providers[1]["system_identifier"],
    providers[1]["database_oid"], providers[1]["database_name"],
)
PY
)" || {
  write_static incomplete native_driver_receipt_invalid "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
}
if ! read -r dump_storage_id dump_file_id report_storage_id report_file_id \
    recovery_storage_id recovery_file_id recovery_record_name \
    receipt_report_digest receipt_report_size receipt_dump_digest receipt_dump_size \
    receipt_source_system_id receipt_source_database_oid receipt_source_database_name \
    receipt_target_system_id receipt_target_database_oid receipt_target_database_name \
    <<<"$receipt_fields"; then
  write_static incomplete native_driver_receipt_invalid "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi
dump_retained_fd=""
report_retained_fd=""
recovery_retained_fd=""
if ! retain_receipt_artifact source.dump "$dump_storage_id" "$dump_file_id" dump_retained_fd \
    || ! retain_receipt_artifact native-restore-report.json \
      "$report_storage_id" "$report_file_id" report_retained_fd \
    || ! retain_receipt_artifact "$recovery_record_name" \
      "$recovery_storage_id" "$recovery_file_id" recovery_retained_fd; then
  write_static incomplete native_driver_publication_authority_failed "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi
if [[ -n "${WORLDSTREAM_NATIVE_PG_TEST_AFTER_RECEIPT_RETENTION:-}" ]]; then
  if ! "${WORLDSTREAM_NATIVE_PG_TEST_AFTER_RECEIPT_RETENTION}" "$temp_root_path"; then
    write_static incomplete native_driver_receipt_retention_hook_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
fi
concurrency_observed=false
concurrency_target_limit=0
concurrency_target_backends=0
concurrency_keeper_backends=0
concurrency_restore_backends=0
concurrency_distinct_pids=false
concurrency_final_role_sessions=-1
concurrency_final_role_exists=-1
captured_restore_role=""
concurrency_keeper_pid=""
concurrency_restore_pid=""
if [[ "$owned_docker" -eq 1 ]]; then
  target_seal_observation=""
  open_artifact_fd target-seal.stderr target_seal_stderr_fd 1
  if ! target_seal_observation="$("$docker_bin" exec \
    --env PGPASSFILE=/run/secrets/worldstream-pgpass "$target_container" \
    psql --no-password --host 127.0.0.1 --port 5432 --dbname "$target_db" \
    --username "$target_user" --quiet --tuples-only --no-align --set ON_ERROR_STOP=1 \
    --command "SELECT datconnlimit::text || '|' || COALESCE(shobj_description(oid, 'pg_database'), '') FROM pg_database WHERE datname = current_database()" \
    2>&"$target_seal_stderr_fd")"; then
    close_artifact_fd "$target_seal_stderr_fd"
    write_static incomplete native_target_seal_observation_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  close_artifact_fd "$target_seal_stderr_fd"
  if [[ "$target_seal_observation" != "0|worldstream/native-postgres-disposable-target/v1" ]]; then
    write_static incomplete native_target_not_durably_sealed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  concurrency_line="$(read_artifact restore-concurrency.observation)"
  IFS='|' read -r captured_restore_role concurrency_target_limit \
    concurrency_target_backends concurrency_keeper_backends \
    concurrency_restore_backends concurrency_restore_login \
    concurrency_restore_super concurrency_restore_limit \
    concurrency_keeper_pid concurrency_restore_pid <<<"$concurrency_line"
  if [[ ! "$captured_restore_role" =~ ^worldstream_restore_[0-9a-f]{32}$ \
      || "$concurrency_target_limit" != "2" \
      || "$concurrency_target_backends" != "2" \
      || "$concurrency_keeper_backends" != "1" \
      || "$concurrency_restore_backends" != "1" \
      || "$concurrency_restore_login" != "true" \
      || "$concurrency_restore_super" != "false" \
      || "$concurrency_restore_limit" != "1" \
      || ! "$concurrency_keeper_pid" =~ ^[1-9][0-9]*$ \
      || ! "$concurrency_restore_pid" =~ ^[1-9][0-9]*$ \
      || "$concurrency_keeper_pid" == "$concurrency_restore_pid" ]]; then
    write_static incomplete native_restore_live_concurrency_observation_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  concurrency_observed=true
  concurrency_distinct_pids=true
  final_restore_observation=""
  open_artifact_fd target-restore-role-cleanup.stderr target_role_cleanup_stderr_fd 1
  if ! final_restore_observation="$($docker_bin exec \
      --env PGPASSFILE=/run/secrets/worldstream-pgpass "$target_container" \
      psql --no-password --host 127.0.0.1 --port 5432 --dbname "$target_db" \
      --username "$target_user" --quiet --tuples-only --no-align --set ON_ERROR_STOP=1 \
      --command "SELECT db.datconnlimit::text || '|' || (SELECT count(*) FROM pg_stat_activity WHERE usename='$captured_restore_role')::text || '|' || (SELECT count(*) FROM pg_roles WHERE rolname='$captured_restore_role')::text || '|' || (SELECT count(*) FROM pg_roles WHERE rolname LIKE 'worldstream_restore_%')::text FROM pg_database db WHERE db.datname=current_database()" \
      2>&"$target_role_cleanup_stderr_fd")"; then
    close_artifact_fd "$target_role_cleanup_stderr_fd"
    write_static incomplete native_restore_credential_cleanup_observation_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  close_artifact_fd "$target_role_cleanup_stderr_fd"
  if [[ "$final_restore_observation" != "0|0|0|0" ]]; then
    write_static incomplete native_restore_credential_cleanup_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  concurrency_final_role_sessions=0
  concurrency_final_role_exists=0
  close_artifact_fd "$restore_input_fd"
  close_artifact_fd "$restore_observation_fd"
fi
if ! last_line="$("$python_bin" - "$report_retained_fd" "$dump_retained_fd" \
  "$receipt_report_digest" "$receipt_report_size" \
  "$receipt_dump_digest" "$receipt_dump_size" \
  "$receipt_source_system_id" "$receipt_source_database_oid" \
  "$receipt_source_database_name" "$receipt_target_system_id" \
  "$receipt_target_database_oid" "$receipt_target_database_name" \
  "$root_dir/scripts/release-platform-diagnostic.py" \
  "$concurrency_observed" \
  "$concurrency_target_limit" "$concurrency_target_backends" \
  "$concurrency_keeper_backends" "$concurrency_restore_backends" \
  "$concurrency_distinct_pids" "$concurrency_final_role_sessions" \
  "$concurrency_final_role_exists" "$captured_restore_role" \
  "$concurrency_keeper_pid" "$concurrency_restore_pid" <<'PY'
import importlib.util
import json
import os
import stat
import sys

report_fd = int(sys.argv[1])
dump_fd = int(sys.argv[2])
expected_report_digest = sys.argv[3]
expected_report_size = int(sys.argv[4])
expected_dump_digest = sys.argv[5]
expected_dump_size = int(sys.argv[6])
expected_source_provider = {
    "system_identifier": sys.argv[7],
    "database_oid": sys.argv[8],
    "database_name": sys.argv[9],
}
expected_target_provider = {
    "system_identifier": sys.argv[10],
    "database_oid": sys.argv[11],
    "database_name": sys.argv[12],
}

spec = importlib.util.spec_from_file_location(
    "worldstream_native_restore_smoke_validator", sys.argv[13]
)
if spec is None or spec.loader is None:
    raise RuntimeError("native report validator is unavailable")
validator = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = validator
spec.loader.exec_module(validator)

blake3_spec = importlib.util.spec_from_file_location(
    "worldstream_native_restore_smoke_streaming_blake3",
    validator.BLAKE3_IMPLEMENTATION_PATH,
)
if blake3_spec is None or blake3_spec.loader is None:
    raise RuntimeError("streaming BLAKE3 verifier is unavailable")
blake3_implementation = importlib.util.module_from_spec(blake3_spec)
sys.modules[blake3_spec.name] = blake3_implementation
blake3_spec.loader.exec_module(blake3_implementation)

MAX_NATIVE_DUMP_BYTES = 8 * 1024 * 1024 * 1024
if expected_report_size > 32 * 1024 * 1024:
    raise RuntimeError("native report exceeds its reviewed bound")
if not 0 < expected_dump_size <= MAX_NATIVE_DUMP_BYTES:
    raise RuntimeError("native dump exceeds its reviewed bound")
report_metadata = os.fstat(report_fd)
dump_metadata = os.fstat(dump_fd)
if (
    not stat.S_ISREG(report_metadata.st_mode)
    or report_metadata.st_nlink != 1
    or report_metadata.st_size != expected_report_size
    or not stat.S_ISREG(dump_metadata.st_mode)
    or dump_metadata.st_nlink != 1
    or dump_metadata.st_size != expected_dump_size
):
    raise RuntimeError("native receipt artifact size or type changed")


def read_exact(descriptor: int, byte_count: int) -> bytes:
    captured = bytearray()
    while len(captured) < byte_count:
        chunk = os.read(descriptor, byte_count - len(captured))
        if not chunk:
            raise RuntimeError("retained native dump ended before its receipt bound")
        captured.extend(chunk)
    return bytes(captured)


def streaming_blake3(descriptor: int, byte_count: int) -> str:
    # The reviewed dependency-free verifier exposes the BLAKE3 chunk and
    # parent primitives. Fold completed 1 KiB chunks into a logarithmic stack,
    # retaining only the final Output so ROOT is applied exactly once.
    os.lseek(descriptor, 0, os.SEEK_SET)
    remaining = byte_count
    chunk_index = 0
    chaining_stack = []
    while remaining > 1024:
        chunk = read_exact(descriptor, 1024)
        chaining_value = blake3_implementation._chunk_output(
            chunk, chunk_index
        ).chaining_value_bytes()
        total_chunks = chunk_index + 1
        while total_chunks & 1 == 0:
            chaining_value = blake3_implementation._parent_output(
                chaining_stack.pop(), chaining_value
            ).chaining_value_bytes()
            total_chunks >>= 1
        chaining_stack.append(chaining_value)
        chunk_index += 1
        remaining -= 1024
    output = blake3_implementation._chunk_output(
        read_exact(descriptor, remaining), chunk_index
    )
    while chaining_stack:
        output = blake3_implementation._parent_output(
            chaining_stack.pop(), output.chaining_value_bytes()
        )
    if os.read(descriptor, 1):
        raise RuntimeError("retained native dump grew beyond its receipt bound")
    return output.root_bytes().hex()


dump_guard = (
    dump_metadata.st_dev,
    dump_metadata.st_ino,
    dump_metadata.st_size,
    dump_metadata.st_nlink,
    dump_metadata.st_mtime_ns,
    dump_metadata.st_ctime_ns,
)
observed_dump_digest = streaming_blake3(dump_fd, expected_dump_size)
dump_after_hash = os.fstat(dump_fd)
if dump_guard != (
    dump_after_hash.st_dev,
    dump_after_hash.st_ino,
    dump_after_hash.st_size,
    dump_after_hash.st_nlink,
    dump_after_hash.st_mtime_ns,
    dump_after_hash.st_ctime_ns,
) or observed_dump_digest != expected_dump_digest:
    raise RuntimeError("native receipt does not bind the retained dump bytes")

os.lseek(report_fd, 0, os.SEEK_SET)
remaining = expected_report_size + 1
chunks = []
while remaining:
    chunk = os.read(report_fd, min(remaining, 64 * 1024))
    if not chunk:
        break
    chunks.append(chunk)
    remaining -= len(chunk)
raw_report = b"".join(chunks)
if (
    len(raw_report) != expected_report_size
    or not raw_report.endswith(b"\n")
    or raw_report.count(b"\n") != 1
    or "blake3:" + validator.BLAKE3(raw_report).hex() != expected_report_digest
):
    raise RuntimeError("native receipt does not bind the retained report bytes")
native_report = json.loads(raw_report)
canonical = json.dumps(
    native_report, ensure_ascii=False, separators=(",", ":")
).encode("utf-8") + b"\n"
if canonical != raw_report:
    raise RuntimeError("native report is not one canonical JSON line")
if (
    native_report.get("native_dump_digest") != expected_dump_digest
    or native_report.get("native_dump_size_bytes") != expected_dump_size
    or native_report.get("source_provider_identity") != expected_source_provider
    or native_report.get("target_provider_identity") != expected_target_provider
):
    raise RuntimeError("native receipt does not bind the report identities")
validator.verify_native_restore_report(native_report)

def exact_bool(value: str) -> bool:
    if value not in {"true", "false"}:
        raise RuntimeError("native concurrency boolean is malformed")
    return value == "true"

live_restore_concurrency = {
    "observed": exact_bool(sys.argv[14]),
    "target_connection_limit_during_restore": int(sys.argv[15]),
    "target_client_backends_excluding_observer": int(sys.argv[16]),
    "direct_superuser_keeper_backends": int(sys.argv[17]),
    "one_use_restore_role_backends": int(sys.argv[18]),
    "keeper_and_restore_pids_distinct": exact_bool(sys.argv[19]),
    "final_captured_role_sessions_across_cluster": int(sys.argv[20]),
    "final_captured_role_exists": int(sys.argv[21]),
    "captured_restore_role": sys.argv[22] or None,
    "captured_keeper_pid": int(sys.argv[23]) if sys.argv[23] else None,
    "captured_restore_pid": int(sys.argv[24]) if sys.argv[24] else None,
}
wrapper = {
    "schema": "worldstream/native-postgres-restore-smoke-evidence/v1",
    "status": "ready",
    "reason": "native_postgres_restore_smoke_completed",
    "exit_code": 0,
    "release_evidence": False,
    "native_restore": native_report,
    "live_restore_concurrency": live_restore_concurrency,
    "target_isolated": native_report["target_isolated"],
    "target_published": native_report["target_published"],
    "secrets_emitted": False,
}
print(json.dumps(wrapper, sort_keys=True, separators=(",", ":")))
PY
)"; then
  write_static incomplete native_driver_report_invalid "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi
if ! cleanup_resources; then
  trap - EXIT
  write_static incomplete native_cleanup_failed "$EXIT_CLEANUP"
  exit "$EXIT_CLEANUP"
fi
trap - EXIT
close_artifact_fd "$dump_retained_fd"
close_artifact_fd "$report_retained_fd"
close_artifact_fd "$recovery_retained_fd"
if [[ -n "$evidence_file" ]]; then
  publish_evidence "$last_line"
fi
printf '%s\n' "$last_line"
if [[ "$driver_code" -eq 0 ]]; then exit "$EXIT_PASS"; fi
exit "$EXIT_INCOMPLETE"
