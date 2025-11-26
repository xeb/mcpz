# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build (LTO + stripped)
cargo test               # Run all tests
cargo install --path .   # Install locally
```

Or use the Makefile:
```bash
make build      # Debug build
make release    # Release build
make test       # Run tests
make install    # Install globally
make publish    # Bump version and publish to crates.io
```

## Architecture

Single-file CLI application (`src/main.rs`) that routes MCP server packages to the correct package manager (npx/uvx/cargo).

### Core Flow

1. **Package Discovery**: Searches crates.io, PyPI, and npm APIs for packages
2. **Popularity Sorting**: Results sorted by download count (most popular first)
3. **Cache**: User selections stored in `~/.cache/mcpz/package_mapping.toml`
4. **Execution**: Runs via `npx -y`, `uvx`, or `cargo install` + binary execution

### Key Types

- `PackageType` - Enum: `Cargo`, `Python`, `Npm` with runner/install info
- `PackageInfo` - Package metadata including downloads count
- `PackageCache` - TOML-serialized HashMap mapping search terms to (package_name, type)

### Registry APIs

- **crates.io**: `https://crates.io/api/v1/crates?q={query}` (requires User-Agent)
- **PyPI**: `https://pypi.org/pypi/{package}/json` + `https://pypistats.org/api/packages/{package}/recent`
- **npm**: `npm search --json` CLI + `https://api.npmjs.org/downloads/point/last-month/{package}`

### CLI Commands

- `run <package> [--first]` - Run package (prompts if multiple matches, `--first` picks most popular)
- `search <package>` - Non-interactive search display
- `pick <package>` - Interactive selection saved to cache
- `clear-cache` - Remove cached mappings

### Detection Logic

1. Packages starting with `@` → npm only
2. Otherwise: search all registries for exact matches
3. If multiple exact matches: prompt user (or pick most popular with `--first`)
4. Single match: use automatically
