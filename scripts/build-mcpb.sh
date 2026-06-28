#!/usr/bin/env bash
# Build a multi-platform MCPB bundle for the Open mSupply MCP server.
#
# MCPB's platform_overrides key on `process.platform` (darwin/linux/win32) — it
# has no arch-level dispatch. So on macOS we fuse arm64+x64 into a universal
# binary via `lipo`; on Linux we ship the host arch only (x64 is conventional
# for redistribution); on Windows we ship x64.
#
# Requires: rustup + `mcpb` CLI (npm i -g @anthropic-ai/mcpb). Cross-compiling
# for Linux/Windows from macOS typically needs `cross` or a suitable linker;
# targets that fail to build are skipped with a warning.

set -euo pipefail

cd "$(dirname "$0")/.."

BIN="omsupply-mcp-server"

# Version is sourced from Cargo.toml (single source of truth). We stamp it into
# manifest.json so the packed MCPB always matches the crate version, and embed it
# in the output filename. Bump the version in one place: Cargo.toml.
VERSION="$(grep -m1 '^version[[:space:]]*=' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
if [ -z "${VERSION}" ]; then
  echo "ERROR: could not read version from Cargo.toml" >&2
  exit 1
fi
echo "==> Version ${VERSION} (from Cargo.toml)"

# Sync the version into manifest.json (rewrites the "version" field in place).
python3 - "$VERSION" <<'PY'
import json, sys
version = sys.argv[1]
with open("manifest.json") as f:
    manifest = json.load(f)
if manifest.get("version") != version:
    manifest["version"] = version
    with open("manifest.json", "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"    manifest.json version -> {version}")
else:
    print("    manifest.json already in sync")
PY

rm -rf bin
mkdir -p bin

build_target() {
  local target="$1"
  if ! rustup target list --installed | grep -q "^${target}$"; then
    echo "    skipping ${target}: rustup target not installed"
    return 1
  fi
  if ! cargo build --release --target "$target"; then
    echo "    skipping ${target}: build failed"
    return 1
  fi
  return 0
}

# macOS — universal binary if both arches build, else whichever did.
echo "==> macOS"
mac_built=()
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  if build_target "$t"; then mac_built+=("target/$t/release/$BIN"); fi
done
if [ ${#mac_built[@]} -eq 2 ]; then
  mkdir -p bin/darwin
  lipo -create -output "bin/darwin/$BIN" "${mac_built[@]}"
elif [ ${#mac_built[@]} -eq 1 ]; then
  mkdir -p bin/darwin
  cp "${mac_built[0]}" "bin/darwin/$BIN"
fi
[ -f "bin/darwin/$BIN" ] && chmod +x "bin/darwin/$BIN"

# Linux x64
echo "==> Linux x64"
if build_target x86_64-unknown-linux-gnu; then
  mkdir -p bin/linux
  cp "target/x86_64-unknown-linux-gnu/release/$BIN" "bin/linux/$BIN"
  chmod +x "bin/linux/$BIN"
fi

# Windows x64
echo "==> Windows x64"
if build_target x86_64-pc-windows-msvc; then
  mkdir -p bin/win32
  cp "target/x86_64-pc-windows-msvc/release/$BIN.exe" "bin/win32/$BIN.exe"
fi

echo "==> Packing MCPB"
OUT="open-msupply-${VERSION}.MCPB"
mcpb pack . "$OUT"
# Also refresh the unversioned "latest" bundle for a stable reference path.
cp "$OUT" open-msupply.MCPB
echo "Done: ${OUT} (also copied to open-msupply.MCPB)"
