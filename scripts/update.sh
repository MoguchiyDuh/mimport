#!/usr/bin/env bash
set -euo pipefail

repo="MoguchiyDuh/mimport"
target="x86_64-unknown-linux-musl"
asset="mimport-${target}.tar.gz"
dest="${MIMPORT_BIN:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/mimport}"
tag="${1:-latest}"

if [[ "$tag" == "latest" ]]; then
    base="https://github.com/${repo}/releases/latest/download"
else
    base="https://github.com/${repo}/releases/download/${tag}"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL --retry 3 -o "$tmp/$asset" "$base/$asset"
curl -fsSL --retry 3 -o "$tmp/$asset.sha256" "$base/$asset.sha256"
(cd "$tmp" && sha256sum -c "$asset.sha256")
tar -C "$tmp" -xzf "$tmp/$asset" mimport
install -m 0755 "$tmp/mimport" "$dest.new"
mv -f "$dest.new" "$dest"
"$dest" --version 2>/dev/null || true
