# mcpz

**This program should not exist.**

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

Shows all matching packages across all registries with version, description, author, publish date, and **download counts**:

```
[1] mcp-server-filesystem v0.1.2
    Registry:    crates.io
    Description: A comprehensive MCP server for filesystem operations
    Author:      See crates.io
    Published:   2025-09-22
    Downloads:   816

[2] mcp-server-filesystem v0.1.0
    Registry:    PyPI
    Description: poneglyph
    Author:      Your Name
    Published:   2025-10-23
    Downloads:   3.7K

[3] @modelcontextprotocol/server-filesystem v2025.11.25
    Registry:    npm
    Description: MCP server for filesystem access
    Author:      pcarleton
    Published:   2025-11-25
    Downloads:   474.9K   <-- probably pick this one
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
