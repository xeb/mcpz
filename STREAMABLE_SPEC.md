# Streamable HTTP Transport Design for mcpz

## Overview

This document describes the design for hosting mcpz's built-in MCP servers (shell, filesystem) over Streamable HTTP transport, as specified in the MCP 2025-03-26 protocol.

## CLI Design

### Basic Usage

```bash
# stdio transport (default, unchanged behavior)
mcpz server shell
mcpz server filesystem -d ~/projects

# HTTP transport - add --http flag
mcpz server shell --http
mcpz server filesystem --http

# Custom port (default: 3000)
mcpz server filesystem --http --port 8080
mcpz server filesystem --http -p 8080

# Custom bind address (default: 127.0.0.1)
mcpz server filesystem --http --host 0.0.0.0

# HTTPS with auto-generated self-signed certificate
mcpz server shell --http --tls

# HTTPS with custom certificate
mcpz server shell --http --tls --cert /path/to/cert.pem --key /path/to/key.pem

# Server-specific options work with both transports
mcpz server filesystem --http -d /home/user/projects -d /tmp
mcpz server shell --http --allow "ls*,cat*" --deny "rm*,sudo*"

# Combined examples
mcpz server filesystem --http -p 8443 --tls -d /data
mcpz server shell --http -p 3001 --tls --allow "git*" --working-dir /repo
```

### Full Command Specification

```
mcpz server <SERVER_TYPE> [OPTIONS]

ARGUMENTS:
  <SERVER_TYPE>    The server to run [possible values: shell, filesystem]

TRANSPORT OPTIONS:
      --http               Use HTTP transport instead of stdio
  -p, --port <PORT>        Port to listen on (HTTP only) [default: 3000]
      --host <HOST>        Address to bind to (HTTP only) [default: 127.0.0.1]

TLS OPTIONS (HTTP only):
      --tls                Enable HTTPS
                           Without --cert/--key: auto-generate self-signed certificate
                           With --cert/--key: use provided certificate
      --cert <PATH>        Path to TLS certificate (PEM format)
      --key <PATH>         Path to TLS private key (PEM format)

HTTP OPTIONS:
      --no-session         Disable session management
      --origin <ORIGINS>   Allowed origins (comma-separated)

COMMON OPTIONS:
  -v, --verbose            Enable verbose logging

FILESYSTEM OPTIONS:
  -d, --directory <PATH>   Allowed directory (can be repeated) [default: .]

SHELL OPTIONS:
      --working-dir <PATH>    Working directory for commands
      --timeout <SECONDS>     Command timeout [default: 30]
      --shell <PATH>          Shell to use [default: /bin/sh]
      --allow <PATTERNS>      Allowed command patterns (comma-separated)
      --deny <PATTERNS>       Denied command patterns (comma-separated)
      --no-stderr             Exclude stderr from output
```

### Help Output

```
$ mcpz server --help
Run a built-in MCP server

Usage: mcpz server <SERVER_TYPE> [OPTIONS]

Available servers:
  filesystem    Filesystem operations (read, write, search, etc.)
  shell         Execute shell commands with sandboxing

Transport:
  By default, servers use stdio transport for MCP client integration.
  Use --http to run as an HTTP server for remote access.

Options:
      --http             Use HTTP transport instead of stdio
  -p, --port <PORT>      HTTP port [default: 3000]
      --host <HOST>      HTTP bind address [default: 127.0.0.1]
      --tls              Enable HTTPS (auto-generates self-signed cert if no --cert/--key)
      --cert <PATH>      TLS certificate path
      --key <PATH>       TLS private key path
  -v, --verbose          Verbose logging

Examples:
  mcpz server filesystem -d ~/projects           # stdio, for MCP clients
  mcpz server shell --http                       # HTTP on localhost:3000
  mcpz server filesystem --http -p 8080 --tls    # HTTPS with self-signed cert
  mcpz server shell --http --tls --cert c.pem --key k.pem  # HTTPS with custom cert
```

## TLS / Self-Signed Certificates

### Behavior

| Flags | Behavior |
|-------|----------|
| (none) | HTTP, no encryption |
| `--tls` | HTTPS with auto-generated self-signed certificate |
| `--tls --cert X --key Y` | HTTPS with provided certificate |
| `--cert X --key Y` (no --tls) | Error: --tls required |

### Self-Signed Certificate Generation

When `--tls` is used without `--cert`/`--key`:

1. Check for cached certificate at `~/.cache/mcpz/tls/self-signed.{crt,key}`
2. If not exists or expired (>365 days), generate new certificate:
   - Subject: `CN=localhost`
   - SAN: `localhost`, `127.0.0.1`, `::1`
   - Validity: 365 days
   - Key: ECDSA P-256
3. Print fingerprint on startup for verification:
   ```
   [mcpz] HTTPS enabled with self-signed certificate
   [mcpz] Certificate fingerprint (SHA-256): AB:CD:12:34:...
   [mcpz] Listening on https://127.0.0.1:3000/mcp
   ```

### Implementation

```rust
use rcgen::{Certificate, CertificateParams, DnType, SanType};

pub struct TlsConfig {
    pub cert_pem: String,
    pub key_pem: String,
}

impl TlsConfig {
    /// Load from files or generate self-signed
    pub fn load_or_generate(
        cert_path: Option<&Path>,
        key_path: Option<&Path>,
    ) -> Result<Self> {
        match (cert_path, key_path) {
            (Some(cert), Some(key)) => Self::load_from_files(cert, key),
            (None, None) => Self::load_or_generate_self_signed(),
            _ => Err(anyhow!("Both --cert and --key must be provided together")),
        }
    }

    fn load_or_generate_self_signed() -> Result<Self> {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mcpz/tls");
        let cert_path = cache_dir.join("self-signed.crt");
        let key_path = cache_dir.join("self-signed.key");

        // Check if cached cert exists and is valid
        if cert_path.exists() && key_path.exists() {
            if let Ok(config) = Self::load_from_files(&cert_path, &key_path) {
                if !Self::is_expired(&config.cert_pem)? {
                    return Ok(config);
                }
            }
        }

        // Generate new self-signed certificate
        let config = Self::generate_self_signed()?;

        // Cache it
        std::fs::create_dir_all(&cache_dir)?;
        std::fs::write(&cert_path, &config.cert_pem)?;
        std::fs::write(&key_path, &config.key_pem)?;

        Ok(config)
    }

    fn generate_self_signed() -> Result<Self> {
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, "localhost");
        params.subject_alt_names = vec![
            SanType::DnsName("localhost".to_string()),
            SanType::IpAddress(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
            SanType::IpAddress(IpAddr::V6(Ipv6Addr::LOCALHOST)),
        ];

        let cert = Certificate::from_params(params)?;

        Ok(Self {
            cert_pem: cert.serialize_pem()?,
            key_pem: cert.serialize_private_key_pem(),
        })
    }
}
```

### Dependencies for TLS

```toml
[dependencies]
rcgen = "0.13"              # Self-signed cert generation
rustls = "0.23"
rustls-pemfile = "2"
tokio-rustls = "0.26"
axum-server = { version = "0.7", features = ["tls-rustls"] }
```

## Architecture

### Component Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                    mcpz server --http                           │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                    HTTP Server (axum)                     │   │
│  │  ┌─────────────────────────────────────────────────────┐ │   │
│  │  │              Request Handler                         │ │   │
│  │  │  POST /mcp  → JSON-RPC dispatch → Response/SSE      │ │   │
│  │  │  GET  /mcp  → Open SSE stream                       │ │   │
│  │  │  DELETE /mcp → Terminate session                    │ │   │
│  │  └─────────────────────────────────────────────────────┘ │   │
│  └──────────────────────────────────────────────────────────┘   │
│                              │                                   │
│  ┌───────────────────────────┴───────────────────────────────┐  │
│  │                   Session Manager                          │  │
│  │  - Session ID generation (UUID v4)                        │  │
│  │  - Session state tracking                                 │  │
│  │  - TTL-based expiration                                   │  │
│  └───────────────────────────────────────────────────────────┘  │
│                              │                                   │
│  ┌───────────────────────────┴───────────────────────────────┐  │
│  │                    MCP Server Adapter                      │  │
│  │  Wraps existing McpServer trait implementations:          │  │
│  │  - FilesystemServer                                       │  │
│  │  - ShellServer                                            │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

### Request Flow

```
Client                           Server
  │                                │
  │  POST /mcp                     │
  │  Content-Type: application/json│
  │  Accept: application/json,     │
  │          text/event-stream     │
  │  Body: {"jsonrpc":"2.0",       │
  │         "method":"initialize", │
  │         "id":1,"params":{}}    │
  │ ─────────────────────────────► │
  │                                │  Validate Origin header
  │                                │  Parse JSON-RPC request
  │                                │  Dispatch to McpServer
  │                                │  Generate session ID
  │                                │
  │  HTTP 200                      │
  │  Content-Type: application/json│
  │  Mcp-Session-Id: <uuid>        │
  │  Body: {"jsonrpc":"2.0",       │
  │         "id":1,"result":{...}} │
  │ ◄───────────────────────────── │
  │                                │
  │  POST /mcp                     │
  │  Mcp-Session-Id: <uuid>        │
  │  Body: {"jsonrpc":"2.0",       │
  │         "method":"tools/call", │
  │         "id":2,"params":{...}} │
  │ ─────────────────────────────► │
  │                                │  Validate session
  │                                │  Execute tool
  │                                │
  │  HTTP 200 (SSE or JSON)        │
  │ ◄───────────────────────────── │
```

### SSE Streaming Response

For long-running tool calls, the server may respond with SSE:

```
HTTP/1.1 200 OK
Content-Type: text/event-stream
Mcp-Session-Id: abc123

data: {"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":50}}

data: {"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"..."}]}}
```

## Implementation Details

### Dependencies

```toml
[dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }

# HTTP server
axum = "0.7"
axum-extra = { version = "0.9", features = ["typed-header"] }
axum-server = { version = "0.7", features = ["tls-rustls"] }
tower = "0.4"
tower-http = { version = "0.5", features = ["cors", "trace"] }

# TLS
rustls = "0.23"
rustls-pemfile = "2"
tokio-rustls = "0.26"
rcgen = "0.13"              # Self-signed cert generation

# Utilities
uuid = { version = "1", features = ["v4"] }
futures = "0.3"
```

### Module Structure

```
src/
├── main.rs              # CLI entry point
├── servers/
│   ├── mod.rs
│   ├── common.rs        # McpServer trait (unchanged)
│   ├── shell.rs         # ShellServer (unchanged)
│   └── filesystem.rs    # FilesystemServer (unchanged)
└── http/
    ├── mod.rs           # HTTP module exports
    ├── server.rs        # Axum server setup, TLS config
    ├── handlers.rs      # POST/GET/DELETE handlers
    ├── session.rs       # Session management
    ├── sse.rs           # SSE stream handling
    ├── tls.rs           # TLS config, self-signed cert generation
    └── middleware.rs    # Origin validation, logging
```

### Key Types

```rust
/// HTTP server configuration
pub struct HttpServerConfig {
    pub port: u16,
    pub host: IpAddr,
    pub tls: Option<TlsConfig>,
    pub allowed_origins: Vec<String>,
    pub session_ttl: Duration,
    pub verbose: bool,
}

/// TLS configuration
pub struct TlsConfig {
    pub cert_pem: String,
    pub key_pem: String,
    pub is_self_signed: bool,
}

/// Session state
pub struct Session {
    pub id: String,
    pub created_at: Instant,
    pub last_activity: Instant,
    pub initialized: bool,
}

/// Session manager
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    ttl: Duration,
}

impl SessionManager {
    pub fn create_session(&self) -> String;
    pub fn validate_session(&self, id: &str) -> Result<(), SessionError>;
    pub fn touch_session(&self, id: &str);
    pub fn delete_session(&self, id: &str);
    pub fn cleanup_expired(&self);
}
```

### Handler Implementation

```rust
/// POST /mcp - Handle JSON-RPC requests
async fn handle_post(
    State(state): State<AppState>,
    TypedHeader(accept): TypedHeader<Accept>,
    session_header: Option<TypedHeader<McpSessionId>>,
    origin: Option<TypedHeader<Origin>>,
    body: String,
) -> Result<Response, StatusCode> {
    // 1. Validate Origin header
    validate_origin(&origin, &state.allowed_origins)?;

    // 2. Parse JSON-RPC request(s)
    let requests = parse_jsonrpc(&body)?;

    // 3. Handle session
    let session_id = match &requests[0] {
        JsonRpcMessage::Request(r) if r.method == "initialize" => {
            // Create new session
            state.sessions.create_session()
        }
        _ => {
            // Validate existing session
            let id = session_header.ok_or(StatusCode::BAD_REQUEST)?.0;
            state.sessions.validate_session(&id)?;
            state.sessions.touch_session(&id);
            id
        }
    };

    // 4. Dispatch to MCP server
    let responses = dispatch_requests(&state.mcp_server, requests);

    // 5. Return response (JSON or SSE based on Accept header)
    if should_use_sse(&accept) {
        Ok(sse_response(session_id, responses))
    } else {
        Ok(json_response(session_id, responses))
    }
}

/// GET /mcp - Open SSE stream for server-initiated messages
async fn handle_get(
    State(state): State<AppState>,
    session_header: TypedHeader<McpSessionId>,
) -> Result<Sse<impl Stream<Item = Event>>, StatusCode> {
    state.sessions.validate_session(&session_header.0)?;

    // Return SSE stream (currently no server-initiated messages needed)
    Ok(Sse::new(empty_stream()))
}

/// DELETE /mcp - Terminate session
async fn handle_delete(
    State(state): State<AppState>,
    session_header: TypedHeader<McpSessionId>,
) -> StatusCode {
    state.sessions.delete_session(&session_header.0);
    StatusCode::OK
}
```

## Security Considerations

### Origin Validation

```rust
fn validate_origin(
    origin: &Option<TypedHeader<Origin>>,
    allowed: &[String],
) -> Result<(), StatusCode> {
    match origin {
        Some(o) => {
            let origin_str = o.to_string();
            // Always allow localhost variants
            if origin_str.starts_with("http://localhost")
                || origin_str.starts_with("http://127.0.0.1")
                || origin_str.starts_with("https://localhost")
                || origin_str.starts_with("https://127.0.0.1") {
                return Ok(());
            }
            // Check against allowed list
            if allowed.contains(&origin_str) || allowed.contains(&"*".to_string()) {
                return Ok(());
            }
            Err(StatusCode::FORBIDDEN)
        }
        // No origin header - likely same-origin or non-browser client
        None => Ok(())
    }
}
```

### Localhost Binding

By default, the server binds to `127.0.0.1` to prevent external access. Users must explicitly use `--host 0.0.0.0` to expose the server, which triggers a warning:

```
WARNING: Binding to 0.0.0.0 exposes this server to all network interfaces.
         Ensure proper authentication and firewall rules are in place.
```

### TLS Recommendations

When binding to non-localhost addresses without TLS:

```
WARNING: Running without TLS on a public interface.
         Consider using --tls for encrypted connections.
```

When using self-signed certificates:

```
[mcpz] Using self-signed certificate (clients will need to trust it)
[mcpz] Fingerprint: SHA256:AB:CD:12:34:56:78:...
```

## Testing

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_initialize_creates_session() {
        let app = create_test_app();
        let response = app
            .oneshot(Request::post("/mcp")
                .header("Accept", "application/json")
                .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("mcp-session-id"));
    }

    #[tokio::test]
    async fn test_request_without_session_fails() {
        let app = create_test_app();
        let response = app
            .oneshot(Request::post("/mcp")
                .header("Accept", "application/json")
                .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_origin_validation_blocks_foreign_origin() {
        let app = create_test_app();
        let response = app
            .oneshot(Request::post("/mcp")
                .header("Origin", "https://evil.com")
                .header("Accept", "application/json")
                .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_self_signed_cert_generation() {
        let config = TlsConfig::generate_self_signed().unwrap();
        assert!(config.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(config.key_pem.contains("BEGIN PRIVATE KEY"));
    }
}
```

### Integration Tests

```bash
# Test HTTP (no TLS)
curl -X POST http://localhost:3000/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  -v

# Test HTTPS with self-signed cert (-k to skip verification)
curl -k -X POST https://localhost:3000/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  -v

# Use session ID from response
curl -X POST http://localhost:3000/mcp \
  -H "Content-Type: application/json" \
  -H "Mcp-Session-Id: <session-id>" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
```

## Startup Output Examples

### HTTP Mode
```
$ mcpz server filesystem --http -d ~/projects
[mcpz] Filesystem server (HTTP)
[mcpz] Allowed directories: /home/user/projects
[mcpz] Listening on http://127.0.0.1:3000/mcp
```

### HTTPS with Self-Signed Cert
```
$ mcpz server shell --http --tls
[mcpz] Shell server (HTTPS)
[mcpz] Using self-signed certificate
[mcpz] Fingerprint: SHA256:AB:CD:12:34:56:78:9A:BC:DE:F0:...
[mcpz] Listening on https://127.0.0.1:3000/mcp
```

### HTTPS with Custom Cert
```
$ mcpz server filesystem --http --tls --cert server.crt --key server.key -p 8443
[mcpz] Filesystem server (HTTPS)
[mcpz] Allowed directories: .
[mcpz] Using certificate: server.crt
[mcpz] Listening on https://127.0.0.1:8443/mcp
```

### Network Exposure Warning
```
$ mcpz server shell --http --host 0.0.0.0
WARNING: Binding to 0.0.0.0 exposes this server to all network interfaces.
         Ensure proper authentication and firewall rules are in place.
WARNING: Running without TLS on a public interface.
         Consider using --tls for encrypted connections.
[mcpz] Shell server (HTTP)
[mcpz] Listening on http://0.0.0.0:3000/mcp
```

## Future Enhancements

1. **Authentication** - Bearer token or API key support via `--auth-token`
2. **Rate Limiting** - Per-session request limits via `--rate-limit`
3. **Metrics** - Prometheus endpoint via `--metrics`
4. **Health Check** - `GET /health` endpoint for load balancers
5. **mTLS** - Client certificate authentication via `--client-ca`
