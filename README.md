# mcpz

[![Crates.io](https://img.shields.io/crates/v/mcpz.svg)](https://crates.io/crates/mcpz)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**This program should not exist. But it does, because MCP should be easy. (MCP-z, get it?)**

There is absolutely no reason for anyone to ever use this. The MCP ecosystem already has perfectly fine tooling. You could just... read the documentation. Or memorize which packages are on npm vs PyPI vs crates.io. Or maintain a spreadsheet. Like a normal person.

**BUT** if you're like me and find yourself:
- Staring blankly at your terminal wondering "was it `npx` or `uvx`?"
- Googling the same MCP package for the 47th time
- Just wanting to write agents that can actually DO things instead of debugging package managers
- Questioning your life choices at 2am because `mcp-server-filesystem` exists in THREE different registries

Then this is for you.

## What it does

`mcpz` is a runtime MCP router that figures out which package manager to use so you don't have to. It searches across **crates.io**, **PyPI**, and **npm** simultaneously, shows you download counts (so you can pick the one that's actually maintained), and caches your choices so you never have to think about it again.

## Installation

```bash
cargo install mcpz
```

Or clone and build:
```bash
git clone https://github.com/xeb/mcpz
cd mcpz
cargo build --release
```

## Usage

### Search for packages

```bash
mcpz search mcp-server-filesystem
```

Shows all matching packages across all registries with version, description, author, publish date, and **download counts** (sorted by popularity):

```
Found 13 packages (sorted by popularity):

[1] @modelcontextprotocol/sdk v1.23.0
    Registry:    npm
    Description: Model Context Protocol implementation for TypeScript
    Author:      pcarleton
    Published:   2025-11-25
    Downloads:   36.1M

[2] @modelcontextprotocol/server-filesystem v2025.11.25
    Registry:    npm
    Description: MCP server for filesystem access
    Author:      pcarleton
    Published:   2025-11-25
    Downloads:   474.9K   <-- the official one, pick this

[3] @latitude-data/supergateway v2.1.4
    Registry:    npm
    Description: Run MCP stdio servers over SSE or visa versa
    Author:      gerardclos
    Published:   2025-03-04
    Downloads:   63.0K

...

[8] mcp-server-filesystem v0.1.2
    Registry:    crates.io
    Description: A comprehensive MCP server for filesystem operations
    Author:      See crates.io
    Published:   2025-09-22
    Downloads:   816
```

### Run a package

```bash
mcpz run mcp-server-time
```

If multiple exact matches exist, you'll be prompted to choose. Your choice is cached for future runs.

### Auto-pick first match

```bash
mcpz run --first mcp-server-filesystem
# or
mcpz run -f mcp-server-filesystem
```

### Pick and save to cache

```bash
mcpz pick mcp-server-filesystem
```

Interactive selection that saves to cache without running.

### Clear cache

```bash
mcpz clear-cache
```

Cache is stored at `~/.cache/mcpz/package_mapping.toml`

### Built-in MCP Shell Server

Run a built-in MCP server for shell command execution:

```bash
mcpz server shell
```

This starts an MCP-compliant server over stdio that LLM clients can use to execute shell commands. Configure it in Claude Desktop:

```json
{
  "mcpServers": {
    "shell": {
      "command": "mcpz",
      "args": ["server", "shell"]
    }
  }
}
```

**With sandboxing** (recommended for production):

```json
{
  "mcpServers": {
    "shell": {
      "command": "mcpz",
      "args": [
        "server", "shell",
        "--working-dir", "/home/user/projects",
        "--allow", "ls*,cat*,grep*,find*",
        "--deny", "rm*,sudo*,chmod*"
      ]
    }
  }
}
```

Options:
- `--working-dir <PATH>` - Restrict execution to a directory
- `--allow <PATTERNS>` - Only allow matching commands (comma-separated, wildcards supported)
- `--deny <PATTERNS>` - Block matching commands (takes precedence over allow)
- `--timeout <SECONDS>` - Command timeout (default: 30)
- `--shell <PATH>` - Shell to use (default: /bin/sh)
- `--verbose` - Enable debug logging to stderr

### Built-in MCP Filesystem Server

```bash
mcpz server filesystem
```

Provides filesystem operations (read, write, list, search, etc.) with directory sandboxing:

```bash
mcpz server filesystem -d /home/user/projects -d /tmp
```

Options:
- `-d, --dir <PATH>` - Allowed directory (can specify multiple times, defaults to current directory)
- `--verbose` - Enable debug logging

### HTTP Transport (Streamable HTTP)

Both built-in servers support HTTP transport in addition to stdio, following the [MCP Streamable HTTP specification](https://modelcontextprotocol.io/specification/2025-03-26/basic/transports#streamable-http):

```bash
# HTTP on localhost:3000
mcpz server shell --http

# Custom port and host
mcpz server shell --http -p 8080 -H 0.0.0.0

# HTTPS with auto-generated self-signed certificate
mcpz server shell --http --tls

# HTTPS with custom certificate
mcpz server filesystem --http --tls --cert /path/to/cert.pem --key /path/to/key.pem
```

HTTP Transport Options:
- `--http` - Use HTTP transport instead of stdio
- `-p, --port <PORT>` - HTTP port (default: 3000)
- `-H, --host <HOST>` - Bind address (default: 127.0.0.1)
- `--tls` - Enable HTTPS (auto-generates self-signed cert if no --cert/--key)
- `--cert <PATH>` - TLS certificate path (use with --key)
- `--key <PATH>` - TLS private key path (use with --cert)
- `--origin <ORIGINS>` - Allowed CORS origins (comma-separated)

Test with curl:
```bash
# Initialize session
curl -X POST http://localhost:3000/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'

# Use mcp-session-id from response header for subsequent requests
```

## How it works

1. **Search order**: crates.io → PyPI → npm
2. **Scoped packages** (like `@modelcontextprotocol/server-filesystem`) go straight to npm
3. **Exact matches** trigger selection if found in multiple registries
4. **Cache** remembers your choices so subsequent runs are instant

## Links

- **crates.io**: [https://crates.io/crates/mcpz](https://crates.io/crates/mcpz)
- **Repository**: [https://github.com/xeb/mcpz](https://github.com/xeb/mcpz)
- **Author**: Mark Kockerbeck

## License

MIT

---

*Now go build some agents instead of fighting with package managers.*
