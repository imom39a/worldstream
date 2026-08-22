#!/bin/sh
set -eu

data_dir="${WORLDSTREAM__STORAGE__DATA_DIR:-/var/lib/worldstream}"
profile="${WORLDSTREAM__STORAGE__PROFILE:-sqlite-bundled}"

case "$profile" in
  sqlite-bundled)
    if [ "$data_dir" != "/var/lib/worldstream" ]; then
      echo 'OCI SQLite policy rejected: data directory must be /var/lib/worldstream' >&2
      exit 78
    fi
    if [ ! -d "$data_dir" ] || [ -L "$data_dir" ]; then
      echo 'OCI SQLite policy rejected: explicit persistent volume is not mounted' >&2
      exit 78
    fi
    mountinfo=/proc/self/mountinfo
    if [ ! -r "$mountinfo" ]; then
      echo 'OCI SQLite policy rejected: mount identity could not be determined' >&2
      exit 78
    fi
    # GNU stat reports the shared ext-family magic as "ext2/ext3" even for
    # ext4, which cannot prove the frozen ext4-only policy. Linux mountinfo
    # carries the kernel's actual filesystem name after the " - " separator.
    mount_identity="$(awk -v path="$data_dir" '
      $5 == path {
        for (field = 6; field <= NF; field += 1) {
          if ($field == "-" && field + 2 <= NF) {
            print $3, $(field + 1), $(field + 2)
          }
        }
      }
    ' "$mountinfo" | tail -n 1)"
    if [ -z "$mount_identity" ]; then
      echo 'OCI SQLite policy rejected: data directory is not an explicit mount' >&2
      exit 78
    fi
    old_ifs="$IFS"
    IFS=' '
    read -r mount_device filesystem mount_source mount_extra <<EOF
$mount_identity
EOF
    IFS="$old_ifs"
    if [ -z "$mount_device" ] || [ -z "$filesystem" ] \
      || [ -z "$mount_source" ] || [ -n "${mount_extra:-}" ]; then
      echo 'OCI SQLite policy rejected: mount identity could not be determined' >&2
      exit 78
    fi
    case "$filesystem" in
      ext4|xfs) ;;
      *)
        echo 'OCI SQLite policy rejected: filesystem is outside the ext4/xfs allow-list' >&2
        exit 78
        ;;
    esac
    case "$mount_device" in
      0:*|*[!0-9:]*|*:*:*|*:)
        echo 'OCI SQLite policy rejected: mount device is not a local block device' >&2
        exit 78
        ;;
      [1-9]*:[0-9]*) ;;
      *)
        echo 'OCI SQLite policy rejected: mount device is not a local block device' >&2
        exit 78
        ;;
    esac
    case "$mount_source" in
      /dev/*) ;;
      *)
        echo 'OCI SQLite policy rejected: mount source is not a local block device' >&2
        exit 78
        ;;
    esac
    ;;
  postgres-primary)
    # PostgreSQL owns persistence outside this image; do not apply the bundled
    # SQLite mount rule, but keep profile selection fail-closed below.
    ;;
  *)
    echo 'OCI storage policy rejected: unsupported storage profile' >&2
    exit 78
    ;;
esac

exec /usr/local/bin/worldstreamd "$@"
