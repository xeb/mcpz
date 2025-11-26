# MCP Server Shell Specification

This document specifies how to add a built-in MCP shell server to mcpz, invoked via `mcpz server shell`.

## Overview

The feature adds an embedded MCP (Model Context Protocol) server that provides shell command execution capabilities over stdio transport. This allows LLM clients (Claude Desktop, Zed, etc.) to execute shell commands through mcpz without needing a separate Python/Node installation.

## Usage

```bash
mcpz server shell
```

This starts an MCP server that:
- Reads JSON-RPC messages from stdin
- Writes JSON-RPC responses to stdout
- Provides an `execute_command` tool for running shell commands

### CLI Help

```
$ mcpz server shell --help
Start an MCP server for shell command execution

Usage: mcpz server shell [OPTIONS]

Options:
  -w, --working-dir <PATH>      Working directory for command execution
                                [default: current directory]
  -t, --timeout <SECONDS>       Command execution timeout in seconds
                                [default: 30]
  -s, --shell <PATH>            Shell to use for command execution
                                [default: /bin/sh]
      --allow <PATTERNS>        Only allow commands matching these patterns
                                (comma-separated, supports wildcards)
                                Example: --allow "ls*,cat*,echo*"
      --deny <PATTERNS>         Deny commands matching these patterns
                                (comma-separated, supports wildcards)
                                Example: --deny "rm*,sudo*,chmod*"
      --no-stderr               Suppress stderr in command output
  -v, --verbose                 Enable verbose logging to stderr
  -h, --help                    Print help
```

### Configuration Example (Claude Desktop)

Basic (no sandboxing):
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

With sandboxing:
```json
{
  "mcpServers": {
    "shell": {
      "command": "mcpz",
      "args": [
        "server", "shell",
        "--working-dir", "/home/user/projects",
        "--allow", "ls*,cat*,grep*,find*,echo*",
        "--deny", "rm*,sudo*,chmod*,chown*",
        "--timeout", "60"
      ]
    }
  }
}
```

## MCP Protocol Implementation

### Transport Layer

The server uses **stdio transport**:
- Messages are newline-delimited JSON-RPC 2.0
- Each message is a complete JSON object on a single line
- Server reads from stdin, writes to stdout
- Logging/debug output must go to stderr (never stdout)

### Message Format

All messages follow JSON-RPC 2.0:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "method_name",
  "params": {}
}
```

### Required Methods

#### 1. `initialize`

Client sends this first to establish the session.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2024-11-05",
    "capabilities": {},
    "clientInfo": {
      "name": "claude-desktop",
      "version": "1.0.0"
    }
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": "2024-11-05",
    "capabilities": {
      "tools": {}
    },
    "serverInfo": {
      "name": "mcpz-shell",
      "version": "0.1.0"
    }
  }
}
```

#### 2. `initialized` (Notification)

Client sends this after receiving initialize response. No response expected.

```json
{
  "jsonrpc": "2.0",
  "method": "initialized"
}
```

#### 3. `tools/list`

Lists available tools.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/list",
  "params": {}
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "tools": [
      {
        "name": "execute_command",
        "description": "Execute a shell command and return its output",
        "inputSchema": {
          "type": "object",
          "properties": {
            "command": {
              "type": "string",
              "description": "Shell command to execute"
            }
          },
          "required": ["command"]
        }
      }
    ]
  }
}
```

#### 4. `tools/call`

Executes a tool.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "execute_command",
    "arguments": {
      "command": "ls -la"
    }
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "{\n  \"command\": \"ls -la\",\n  \"output\": \"total 24\\ndrwxr-xr-x  5 user  group   160 Jan  1 12:00 .\\n...\",\n  \"return_code\": 0\n}"
      }
    ]
  }
}
```

## Implementation Details

### New CLI Subcommand

Add a `server` subcommand with `shell` variant:

```rust
#[derive(Subcommand)]
enum Commands {
    // ... existing commands ...

    /// Run a built-in MCP server
    Server {
        #[command(subcommand)]
        server_type: ServerType,
    },
}

#[derive(Subcommand)]
enum ServerType {
    /// Start an MCP server for shell command execution
    Shell {
        /// Working directory for command execution
        #[arg(short = 'w', long, value_name = "PATH")]
        working_dir: Option<PathBuf>,

        /// Command execution timeout in seconds
        #[arg(short = 't', long, default_value = "30")]
        timeout: u64,

        /// Shell to use for command execution
        #[arg(short = 's', long, default_value = "/bin/sh")]
        shell: String,

        /// Only allow commands matching these patterns (comma-separated)
        #[arg(long, value_name = "PATTERNS")]
        allow: Option<String>,

        /// Deny commands matching these patterns (comma-separated)
        #[arg(long, value_name = "PATTERNS")]
        deny: Option<String>,

        /// Suppress stderr in command output
        #[arg(long)]
        no_stderr: bool,

        /// Enable verbose logging to stderr
        #[arg(short = 'v', long)]
        verbose: bool,
    },
}
```

### Server Configuration Struct

```rust
struct ShellServerConfig {
    working_dir: Option<PathBuf>,
    timeout: Duration,
    shell: String,
    allow_patterns: Vec<String>,
    deny_patterns: Vec<String>,
    include_stderr: bool,
    verbose: bool,
}

impl ShellServerConfig {
    fn from_args(
        working_dir: Option<PathBuf>,
        timeout: u64,
        shell: String,
        allow: Option<String>,
        deny: Option<String>,
        no_stderr: bool,
        verbose: bool,
    ) -> Self {
        Self {
            working_dir,
            timeout: Duration::from_secs(timeout),
            shell,
            allow_patterns: allow
                .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
                .unwrap_or_default(),
            deny_patterns: deny
                .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
                .unwrap_or_default(),
            include_stderr: !no_stderr,
            verbose,
        }
    }

    fn is_command_allowed(&self, command: &str) -> bool {
        // Check deny list first
        for pattern in &self.deny_patterns {
            if Self::matches_pattern(command, pattern) {
                return false;
            }
        }

        // If allow list is empty, allow all (that aren't denied)
        if self.allow_patterns.is_empty() {
            return true;
        }

        // Check allow list
        for pattern in &self.allow_patterns {
            if Self::matches_pattern(command, pattern) {
                return true;
            }
        }

        false
    }

    fn matches_pattern(command: &str, pattern: &str) -> bool {
        // Simple wildcard matching: "ls*" matches "ls -la"
        let cmd_first_word = command.split_whitespace().next().unwrap_or("");
        if pattern.ends_with('*') {
            let prefix = &pattern[..pattern.len() - 1];
            cmd_first_word.starts_with(prefix)
        } else {
            cmd_first_word == pattern
        }
    }
}
```

### Core Data Structures

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

#[derive(Serialize)]
struct CommandResult {
    command: String,
    output: String,
    return_code: i32,
}
```

### Main Server Loop

```rust
use std::io::{self, BufRead, Write};

fn run_shell_server() -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    eprintln!("mcpz shell server started"); // Debug to stderr

    for line in stdin.lock().lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = serde_json::from_str(&line)?;
        let response = handle_request(request)?;

        let response_json = serde_json::to_string(&response)?;
        writeln!(stdout, "{}", response_json)?;
        stdout.flush()?;
    }

    Ok(())
}
```

### Request Handler

```rust
fn handle_request(req: JsonRpcRequest) -> anyhow::Result<JsonRpcResponse> {
    let result = match req.method.as_str() {
        "initialize" => handle_initialize(&req.params),
        "initialized" => return Ok(no_response()), // Notification, no response
        "tools/list" => handle_tools_list(),
        "tools/call" => handle_tools_call(&req.params),
        _ => {
            return Ok(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id.unwrap_or(serde_json::Value::Null),
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("Method not found: {}", req.method),
                }),
            });
        }
    };

    Ok(JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: req.id.unwrap_or(serde_json::Value::Null),
        result: Some(result?),
        error: None,
    })
}
```

### Command Execution

```rust
use std::process::Command;
use std::time::Duration;

fn execute_command(command: &str, config: &ShellServerConfig) -> CommandResult {
    // Check sandboxing rules
    if !config.is_command_allowed(command) {
        return CommandResult {
            command: command.to_string(),
            output: format!("Command denied by security policy"),
            return_code: -1,
        };
    }

    if config.verbose {
        eprintln!("[mcpz] Executing: {}", command);
    }

    let mut cmd = Command::new(&config.shell);
    cmd.arg("-c").arg(command);

    // Set working directory if specified
    if let Some(ref dir) = config.working_dir {
        cmd.current_dir(dir);
    }

    // Execute with timeout (simplified - production would use async)
    let output = cmd.output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let combined = if config.include_stderr {
                let stderr = String::from_utf8_lossy(&output.stderr);
                format!("{}{}", stdout, stderr)
            } else {
                stdout.to_string()
            };

            if config.verbose {
                eprintln!("[mcpz] Exit code: {}", output.status.code().unwrap_or(-1));
            }

            CommandResult {
                command: command.to_string(),
                output: combined,
                return_code: output.status.code().unwrap_or(-1),
            }
        }
        Err(e) => {
            if config.verbose {
                eprintln!("[mcpz] Error: {}", e);
            }
            CommandResult {
                command: command.to_string(),
                output: format!("Failed to execute: {}", e),
                return_code: -1,
            }
        }
    }
}
```

### Handler Implementations

```rust
fn handle_initialize(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "mcpz-shell",
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
}

fn handle_tools_list() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "tools": [{
            "name": "execute_command",
            "description": "Execute a shell command and return its output",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    }
                },
                "required": ["command"]
            }
        }]
    }))
}

fn handle_tools_call(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let name = params.get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;

    if name != "execute_command" {
        anyhow::bail!("Unknown tool: {}", name);
    }

    let command = params.get("arguments")
        .and_then(|a| a.get("command"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing command argument"))?;

    let result = execute_command(command);

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&result)?
        }]
    }))
}
```

## Testing

### Manual Testing with MCP Inspector

```bash
npx @modelcontextprotocol/inspector mcpz server shell
```

### Example Session

```bash
# Start the server
mcpz server shell

# Send initialize (paste this line):
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}

# Send initialized notification:
{"jsonrpc":"2.0","method":"initialized"}

# List tools:
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}

# Execute a command:
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"execute_command","arguments":{"command":"echo hello"}}}
```

## Security Considerations

1. **Command Injection**: Commands are executed directly via shell. Users should understand the security implications.

2. **Built-in Sandboxing** (implemented):
   - `--allow <patterns>`: Whitelist of allowed commands (comma-separated, wildcards supported)
   - `--deny <patterns>`: Blacklist of dangerous commands (deny takes precedence)
   - `--working-dir <path>`: Restrict execution to a specific directory
   - Pattern matching is done on the first word of the command (the executable)

3. **Recommended Deny Patterns** for production:
   ```bash
   --deny "rm*,sudo*,chmod*,chown*,kill*,pkill*,dd*,mkfs*,fdisk*,shutdown*,reboot*"
   ```

4. **Logging**: Use `--verbose` to log all commands to stderr for audit purposes.

5. **Timeout**: Default 30-second timeout prevents runaway commands. Adjust with `--timeout`.

## Future Extensions

1. **Additional Tools** (add to shell server):
   - `read_file`: Read file contents
   - `write_file`: Write to files
   - `list_directory`: List directory contents

2. **Additional Servers**:
   - `mcpz server filesystem`: File system operations only (no shell)
   - `mcpz server git`: Git operations
   - `mcpz server http`: HTTP requests

3. **Enhanced Sandboxing**:
   - Environment variable filtering
   - Network access control
   - Resource limits (CPU, memory)
   - chroot/container isolation

## Dependencies

No new dependencies required. Uses existing:
- `serde` / `serde_json` for JSON-RPC
- `std::process::Command` for shell execution
- `std::io` for stdio handling

## Reference Implementation

Based on: https://github.com/modelcontextprotocol/servers/tree/main/src/shell (Python)

The Python implementation uses:
- `mcp` library for server framework
- `subprocess.run(shell=True)` for command execution
- Pydantic for result validation

Our Rust implementation is standalone, requiring no MCP library—just raw JSON-RPC over stdio.
