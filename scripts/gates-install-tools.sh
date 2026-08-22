#!/usr/bin/env bash
set -euo pipefail

# Install the exact hosted security scanners from byte-pinned upstream release
# assets. This script is for ephemeral CI runners; local hooks never mutate a
# developer toolchain.

readonly GITLEAKS_VERSION="8.29.1"
readonly GITLEAKS_SHA256="e4eb209d04e20339d77122a3bdf9cd41351255cfb27ebcb75e85325e04f88924"
readonly CARGO_AUDIT_VERSION="0.22.2"
readonly CARGO_AUDIT_SHA256="ab28a1bdb54db4d5d8ad5981cf1f959410370b3d28250dbd35f6a44248620e39"

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
  printf '%s\n' 'gates-install-tools.sh supports only hosted Linux x86_64' >&2
  exit 2
fi

install_dir="${RUNNER_TEMP:?RUNNER_TEMP is required}/worldstream-gate-tools"
temporary_dir="$(mktemp -d "${RUNNER_TEMP}/worldstream-gate-tools.XXXXXX")"
trap 'rm -rf "$temporary_dir"' EXIT
mkdir -p "$install_dir"

gitleaks_archive="gitleaks_${GITLEAKS_VERSION}_linux_x64.tar.gz"
cargo_audit_archive="cargo-audit-x86_64-unknown-linux-gnu-v${CARGO_AUDIT_VERSION}.tgz"
curl --fail --silent --show-error --location \
  "https://github.com/gitleaks/gitleaks/releases/download/v${GITLEAKS_VERSION}/${gitleaks_archive}" \
  --output "$temporary_dir/$gitleaks_archive"
curl --fail --silent --show-error --location \
  "https://github.com/rustsec/rustsec/releases/download/cargo-audit/v${CARGO_AUDIT_VERSION}/${cargo_audit_archive}" \
  --output "$temporary_dir/$cargo_audit_archive"
printf '%s  %s\n' "$GITLEAKS_SHA256" "$temporary_dir/$gitleaks_archive" \
  | sha256sum --check --strict
printf '%s  %s\n' "$CARGO_AUDIT_SHA256" "$temporary_dir/$cargo_audit_archive" \
  | sha256sum --check --strict

tar -xzf "$temporary_dir/$gitleaks_archive" -C "$temporary_dir"
tar -xzf "$temporary_dir/$cargo_audit_archive" -C "$temporary_dir"
install -m 0755 "$temporary_dir/gitleaks" "$install_dir/gitleaks"
install -m 0755 \
  "$temporary_dir/cargo-audit-x86_64-unknown-linux-gnu-v${CARGO_AUDIT_VERSION}/cargo-audit" \
  "$install_dir/cargo-audit"

"$install_dir/gitleaks" version | grep -F "$GITLEAKS_VERSION" >/dev/null
"$install_dir/cargo-audit" --version | grep -F "cargo-audit ${CARGO_AUDIT_VERSION}" >/dev/null
printf '%s\n' "$install_dir" >> "${GITHUB_PATH:?GITHUB_PATH is required}"
