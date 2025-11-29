# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build (LTO + stripped)
cargo test               # Run all tests
cargo test test_name     # Run a single test
cargo install --path .   # Install locally
```

Or use the Makefile:
```bash
make build      # Debug build
make release    # Release build
make test       # Run tests
make install    # Install globally
make publish    # Bump version and publish to crates.io
make binary-size  # Show release binary size
```

## Architecture

CLI application with modular structure for MCP server routing and built-in servers.

### File Structure

- `src/main.rs` - CLI entry point, package routing logic
- `src/servers/mod.rs` - Server module exports
- `src/servers/common.rs` - Shared MCP types (`JsonRpcRequest`, `JsonRpcResponse`, `McpServer` trait)
- `src/servers/shell.rs` - Shell command execution server
- `src/servers/filesystem.rs` - Filesystem operations server
- `src/http/mod.rs` - HTTP transport module exports
- `src/http/server.rs` - Axum HTTP server setup, TLS config
- `src/http/handlers.rs` - POST/GET/DELETE endpoint handlers
- `src/http/session.rs` - MCP session management
- `src/http/tls.rs` - TLS config and self-signed certificate generation

### Core Flow (Package Routing)

1. **Package Discovery**: Searches crates.io, PyPI, and npm APIs for packages
2. **Popularity Sorting**: Results sorted by download count (most popular first)
3. **Cache**: User selections stored in `~/.cache/mcpz/package_mapping.toml`
4. **Execution**: Runs via `npx -y`, `uvx`, or `cargo install` + binary execution

### Key Types

- `PackageType` - Enum: `Cargo`, `Python`, `Npm` with runner/install info
- `PackageInfo` - Package metadata including downloads count
- `PackageCache` - TOML-serialized HashMap mapping search terms to (package_name, type)
- `McpServer` trait - Common interface for built-in MCP servers

### Registry APIs

- **crates.io**: `https://crates.io/api/v1/crates?q={query}` (requires User-Agent)
- **PyPI**: `https://pypi.org/pypi/{package}/json` + `https://pypistats.org/api/packages/{package}/recent`
- **npm**: `npm search --json` CLI + `https://api.npmjs.org/downloads/point/last-month/{package}`

### CLI Commands

- `run <package> [--first]` - Run package (prompts if multiple matches, `--first` picks most popular)
- `search <package>` - Non-interactive search display
- `pick <package>` - Interactive selection saved to cache
- `clear-cache` - Remove cached mappings
- `server list` - List available built-in MCP servers
- `server shell` - Run built-in MCP shell server
- `server filesystem` - Run built-in MCP filesystem server

### Built-in MCP Servers

#### Shell Server (`server shell`)
Executes shell commands via JSON-RPC over stdio.
- `ShellServerConfig` - Working directory, timeout, shell path, allow/deny patterns
- Sandboxing via `--allow`/`--deny` patterns (deny takes precedence)
- Single tool: `execute_command`

#### Filesystem Server (`server filesystem`)
Provides filesystem operations with directory sandboxing.
- `FilesystemServerConfig` - Allowed directories list
- Path validation prevents access outside allowed directories (including symlink attacks)
- Tools: `read_file`, `read_multiple_files`, `write_file`, `edit_file`, `create_directory`, `list_directory`, `list_directory_with_sizes`, `directory_tree`, `move_file`, `search_files`, `get_file_info`, `list_allowed_directories`

### HTTP Transport (`--http` flag)

Built-in servers can run over HTTP instead of stdio using the `--http` flag. This implements the MCP Streamable HTTP transport (2025-03-26 spec).

#### HTTP Options
- `--http` - Enable HTTP transport instead of stdio
- `-p, --port <PORT>` - Port to listen on (default: 3000)
- `--host <HOST>` - Bind address (default: 127.0.0.1)
- `--tls` - Enable HTTPS (auto-generates self-signed cert if no --cert/--key)
- `--cert <PATH>` - TLS certificate path (PEM format)
- `--key <PATH>` - TLS private key path (PEM format)
- `--origin <ORIGINS>` - Allowed CORS origins (comma-separated)

#### Examples
```bash
mcpz server shell --http                    # HTTP on localhost:3000
mcpz server filesystem --http -p 8080       # HTTP on port 8080
mcpz server shell --http --tls              # HTTPS with self-signed cert
mcpz server filesystem --http --tls --cert cert.pem --key key.pem  # HTTPS with custom cert
```

#### HTTP Module Structure
- `HttpServerConfig` - Port, host, TLS config, allowed origins
- `TlsConfig` - Certificate/key loading or self-signed generation
- `SessionManager` - UUID-based session tracking with TTL
- `AppState` - Wraps `McpServer` trait for HTTP handlers
- Endpoint: `POST/GET/DELETE /mcp` per MCP Streamable HTTP spec

### Detection Logic

1. Packages starting with `@` → npm only
2. Otherwise: search all registries for exact matches
3. If multiple exact matches: prompt user (or pick most popular with `--first`)
4. Single match: use automatically
