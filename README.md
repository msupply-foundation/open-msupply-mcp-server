# Open mSupply MCP Server

An [MCP (Model Context Protocol)](https://modelcontextprotocol.io) server that
connects AI assistants to [Open mSupply](https://github.com/msupply-foundation/open-msupply)
instances. Query inventory, stock levels, shipments, requisitions, and more
through natural language.

Written in Rust and distributed as a self-contained native binary inside an
[MCPB](https://github.com/anthropics/mcpb) bundle — no Node.js or Python
runtime required on the host.

## Install

Download `open-msupply.MCPB` from Releases and open it with a compatible MCP
host (e.g. Claude Desktop). The host will prompt for server URL, username, and
password on first launch.

## Configuration

Provided via MCPB user config (or the equivalent env vars when running the
binary directly):

| Variable | Required | Description |
|---|---|---|
| `OMSUPPLY_URL` | yes | Open mSupply server URL (e.g. `http://127.0.0.1:8000`) |
| `OMSUPPLY_USERNAME` | yes | Login username |
| `OMSUPPLY_PASSWORD` | yes | Login password |
| `OMSUPPLY_STORE_ID` | no | Default store ID (discover via `list_stores`) |
| `OMSUPPLY_ALLOW_SELF_SIGNED` | no | `true` to accept self-signed TLS certs |

## Build from source

```bash
cargo build --release
./target/release/omsupply-mcp-server
```

## Build an MCPB bundle

The bundle ships per-platform binaries under `bin/<platform>/`. MCPB's
`platform_overrides` dispatches on `process.platform` (`darwin`/`linux`/`win32`)
— it has no arch-level selector, so on macOS the build script fuses
`aarch64` + `x86_64` into a universal binary via `lipo`.

```bash
npm i -g @anthropic-ai/mcpb
./scripts/build-mcpb.sh
```

The script cross-compiles each supported target (skipping any whose rustup
target or linker is unavailable on the current host) and packs the result into
`open-msupply.MCPB`.

Supported targets:

- `darwin-arm64`, `darwin-x64`
- `linux-x64`, `linux-arm64`
- `win32-x64`

## Bundle layout

```
open-msupply.MCPB
├── manifest.json
└── bin/
    ├── darwin/omsupply-mcp-server          # universal (arm64 + x86_64)
    ├── linux/omsupply-mcp-server           # x86_64
    └── win32/omsupply-mcp-server.exe       # x86_64
```

## License

AGPL-3.0
