#!/usr/bin/env bash
set -euo pipefail

version="0.20.2"
destination="${1:-}"

if [[ -z "$destination" ]]; then
  echo "usage: scripts/install_cargo_deny.sh DESTINATION_DIRECTORY" >&2
  exit 2
fi

case "$(uname -s):$(uname -m)" in
  Darwin:arm64)
    target="aarch64-apple-darwin"
    expected_sha256="fe67d82a10d8597a3549364cb733a3f9cc1bfff9031b7ae46384a9f2a72090c3"
    ;;
  Darwin:x86_64)
    target="x86_64-apple-darwin"
    expected_sha256="248da7f581724e470071990c088ffc55c811981715f4cbdb258621fb79f8b7a6"
    ;;
  Linux:aarch64 | Linux:arm64)
    target="aarch64-unknown-linux-musl"
    expected_sha256="995c82be0defc7a025cae49a2aa2644ce8245c9a3318fc4103907c6a285e8c7d"
    ;;
  Linux:x86_64)
    target="x86_64-unknown-linux-musl"
    expected_sha256="9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f"
    ;;
  *)
    echo "unsupported cargo-deny host: $(uname -s) $(uname -m)" >&2
    exit 1
    ;;
esac

temporary="$(mktemp -d "${TMPDIR:-/tmp}/packet28-cargo-deny.XXXXXX")"
trap 'rm -rf -- "$temporary"' EXIT

archive="$temporary/cargo-deny.tar.gz"
url="https://github.com/EmbarkStudios/cargo-deny/releases/download/${version}/cargo-deny-${version}-${target}.tar.gz"
curl --fail --location --silent --show-error \
  --proto '=https' --tlsv1.2 \
  --output "$archive" "$url"

if command -v sha256sum >/dev/null 2>&1; then
  actual_sha256="$(sha256sum "$archive" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual_sha256="$(shasum -a 256 "$archive" | awk '{print $1}')"
else
  echo "sha256sum or shasum is required to verify cargo-deny" >&2
  exit 1
fi

if [[ "$actual_sha256" != "$expected_sha256" ]]; then
  echo "cargo-deny archive checksum mismatch" >&2
  echo "expected: $expected_sha256" >&2
  echo "actual:   $actual_sha256" >&2
  exit 1
fi

tar -xzf "$archive" -C "$temporary"
mkdir -p "$destination"
install -m 0755 \
  "$temporary/cargo-deny-${version}-${target}/cargo-deny" \
  "$destination/cargo-deny"
"$destination/cargo-deny" --version
