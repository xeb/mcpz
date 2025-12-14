use anyhow::{anyhow, Result};
use serde::Serialize;
use sqlx::any::AnyRow;
use sqlx::{AnyPool, Column, Row, TypeInfo};
use std::time::Duration;

use super::common::{error_content, text_content, McpServer, McpTool};

/// Access mode for the SQL server
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// Only SELECT queries allowed
    ReadOnly,
    /// All SQL statements allowed (SELECT, INSERT, UPDATE, DELETE, etc.)
    FullAccess,
}

/// Configuration for the SQL server
pub struct SqlServerConfig {
    pub connection_string: String,
    pub access_mode: AccessMode,
    pub timeout: Duration,
    pub verbose: bool,
}

impl SqlServerConfig {
    pub fn new(connection_string: String, access_mode: AccessMode, timeout: u64, verbose: bool) -> Self {
        Self {
            connection_string,
            access_mode,
            timeout: Duration::from_secs(timeout),
            verbose,
        }
    }

    /// Detect database type from connection string
    pub fn database_type(&self) -> &'static str {
        if self.connection_string.starts_with("postgres://")
            || self.connection_string.starts_with("postgresql://")
        {
            "PostgreSQL"
        } else if self.connection_string.starts_with("mysql://")
            || self.connection_string.starts_with("mariadb://")
        {
            "MySQL"
        } else if self.connection_string.starts_with("sqlite://")
            || self.connection_string.starts_with("sqlite:")
        {
            "SQLite"
        } else {
            "Unknown"
        }
    }

    /// Check if a SQL statement is allowed based on access mode
    pub fn is_statement_allowed(&self, sql: &str) -> bool {
        if self.access_mode == AccessMode::FullAccess {
            return true;
        }

        // In readonly mode, only allow SELECT statements
        let trimmed = sql.trim().to_uppercase();

        // Allow SELECT, WITH (for CTEs that result in SELECT), EXPLAIN, SHOW, DESCRIBE
        trimmed.starts_with("SELECT")
            || trimmed.starts_with("WITH")
            || trimmed.starts_with("EXPLAIN")
            || trimmed.starts_with("SHOW")
            || trimmed.starts_with("DESCRIBE")
            || trimmed.starts_with("DESC")
            || trimmed.starts_with("PRAGMA")  // SQLite introspection
    }
}

/// Query result for serialization
#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub row_count: usize,
}

/// Execute result for non-SELECT statements
#[derive(Debug, Serialize)]
pub struct ExecuteResult {
    pub rows_affected: u64,
    pub message: String,
}

/// Table info for list_tables
#[derive(Debug, Serialize)]
pub struct TableInfo {
    pub name: String,
    pub table_type: String,
}

/// Column info for describe_table
#[derive(Debug, Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
}

/// SQL MCP server
pub struct SqlServer {
    config: SqlServerConfig,
    pool: AnyPool,
    runtime: tokio::runtime::Runtime,
}

impl SqlServer {
    pub fn new(config: SqlServerConfig, pool: AnyPool, runtime: tokio::runtime::Runtime) -> Self {
        Self {
            config,
            pool,
            runtime,
        }
    }

    /// Convert a row to JSON values
    fn row_to_json(row: &AnyRow) -> Vec<serde_json::Value> {
        let mut values = Vec::new();
        for i in 0..row.columns().len() {
            let col = &row.columns()[i];
            let type_name = col.type_info().name();

            // Try to extract value based on type
            let value: serde_json::Value = match type_name.to_uppercase().as_str() {
                "INT" | "INT4" | "INTEGER" | "BIGINT" | "INT8" | "SMALLINT" | "INT2" => {
                    row.try_get::<i64, _>(i)
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null)
                }
                "FLOAT" | "FLOAT4" | "FLOAT8" | "DOUBLE" | "REAL" | "NUMERIC" | "DECIMAL" => {
                    row.try_get::<f64, _>(i)
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null)
                }
                "BOOL" | "BOOLEAN" => {
                    row.try_get::<bool, _>(i)
                        .map(serde_json::Value::from)
                        .unwrap_or(serde_json::Value::Null)
                }
                "NULL" => serde_json::Value::Null,
                _ => {
                    // Default to string for TEXT, VARCHAR, and other types
                    row.try_get::<String, _>(i)
                        .map(serde_json::Value::from)
                        .unwrap_or_else(|_| {
                            // Try as bytes and convert to string
                            row.try_get::<Vec<u8>, _>(i)
                                .map(|b| String::from_utf8_lossy(&b).to_string())
                                .map(serde_json::Value::from)
                                .unwrap_or(serde_json::Value::Null)
                        })
                }
            };
            values.push(value);
        }
        values
    }

    /// Execute a query and return results
    fn execute_query(&self, sql: &str) -> Result<QueryResult> {
        if !self.config.is_statement_allowed(sql) {
            return Err(anyhow!(
                "Statement not allowed in readonly mode. Only SELECT, SHOW, DESCRIBE, and EXPLAIN are permitted."
            ));
        }

        self.log(&format!("Executing query: {}", sql));

        let result = self.runtime.block_on(async {
            let rows: Vec<AnyRow> = sqlx::query(sql).fetch_all(&self.pool).await?;

            if rows.is_empty() {
                return Ok(QueryResult {
                    columns: vec![],
                    rows: vec![],
                    row_count: 0,
                });
            }

            let columns: Vec<String> = rows[0]
                .columns()
                .iter()
                .map(|c| c.name().to_string())
                .collect();

            let json_rows: Vec<Vec<serde_json::Value>> =
                rows.iter().map(Self::row_to_json).collect();
            let row_count = json_rows.len();

            Ok(QueryResult {
                columns,
                rows: json_rows,
                row_count,
            })
        });

        result
    }

    /// Execute a statement (INSERT, UPDATE, DELETE, etc.)
    fn execute_statement(&self, sql: &str) -> Result<ExecuteResult> {
        if self.config.access_mode == AccessMode::ReadOnly {
            return Err(anyhow!(
                "Write operations not allowed in readonly mode. Use --fullaccess to enable."
            ));
        }

        self.log(&format!("Executing statement: {}", sql));

        let result = self.runtime.block_on(async {
            let result = sqlx::query(sql).execute(&self.pool).await?;
            let rows_affected = result.rows_affected();

            Ok(ExecuteResult {
                rows_affected,
                message: format!("Statement executed successfully. {} row(s) affected.", rows_affected),
            })
        });

        result
    }

    /// List all tables in the database
    fn list_tables(&self) -> Result<Vec<TableInfo>> {
        let sql = if self.config.connection_string.starts_with("postgres")
            || self.config.connection_string.starts_with("postgresql")
        {
            "SELECT table_name as name, table_type FROM information_schema.tables WHERE table_schema = 'public' ORDER BY table_name"
        } else if self.config.connection_string.starts_with("mysql")
            || self.config.connection_string.starts_with("mariadb")
        {
            "SELECT table_name as name, table_type FROM information_schema.tables WHERE table_schema = DATABASE() ORDER BY table_name"
        } else {
            // SQLite
            "SELECT name, type as table_type FROM sqlite_master WHERE type IN ('table', 'view') ORDER BY name"
        };

        self.log(&format!("Listing tables with: {}", sql));

        let result = self.runtime.block_on(async {
            let rows: Vec<AnyRow> = sqlx::query(sql).fetch_all(&self.pool).await?;

            let tables: Vec<TableInfo> = rows
                .iter()
                .map(|row| {
                    let name: String = row.try_get("name").unwrap_or_default();
                    let table_type: String = row.try_get("table_type").unwrap_or_else(|_| "TABLE".to_string());
                    TableInfo { name, table_type }
                })
                .collect();

            Ok(tables)
        });

        result
    }

    /// Describe a table's schema
    fn describe_table(&self, table_name: &str) -> Result<Vec<ColumnInfo>> {
        // Sanitize table name to prevent SQL injection
        if !table_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(anyhow!("Invalid table name"));
        }

        let sql = if self.config.connection_string.starts_with("postgres")
            || self.config.connection_string.starts_with("postgresql")
        {
            format!(
                "SELECT column_name as name, data_type, is_nullable FROM information_schema.columns WHERE table_name = '{}' ORDER BY ordinal_position",
                table_name
            )
        } else if self.config.connection_string.starts_with("mysql")
            || self.config.connection_string.starts_with("mariadb")
        {
            format!(
                "SELECT column_name as name, data_type, is_nullable FROM information_schema.columns WHERE table_name = '{}' AND table_schema = DATABASE() ORDER BY ordinal_position",
                table_name
            )
        } else {
            // SQLite - use PRAGMA
            format!("PRAGMA table_info({})", table_name)
        };

        self.log(&format!("Describing table with: {}", sql));

        let result = self.runtime.block_on(async {
            let rows: Vec<AnyRow> = sqlx::query(&sql).fetch_all(&self.pool).await?;

            let columns: Vec<ColumnInfo> = if self.config.connection_string.starts_with("sqlite") {
                // SQLite PRAGMA returns: cid, name, type, notnull, dflt_value, pk
                rows.iter()
                    .map(|row| {
                        let name: String = row.try_get("name").unwrap_or_default();
                        let data_type: String = row.try_get("type").unwrap_or_default();
                        let notnull: i32 = row.try_get("notnull").unwrap_or(0);
                        ColumnInfo {
                            name,
                            data_type,
                            is_nullable: notnull == 0,
                        }
                    })
                    .collect()
            } else {
                rows.iter()
                    .map(|row| {
                        let name: String = row.try_get("name").unwrap_or_default();
                        let data_type: String = row.try_get("data_type").unwrap_or_default();
                        let is_nullable: String = row.try_get("is_nullable").unwrap_or_else(|_| "YES".to_string());
                        ColumnInfo {
                            name,
                            data_type,
                            is_nullable: is_nullable.to_uppercase() == "YES",
                        }
                    })
                    .collect()
            };

            Ok(columns)
        });

        result
    }
}

impl McpServer for SqlServer {
    fn name(&self) -> &str {
        "mcpz-sql"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn verbose(&self) -> bool {
        self.config.verbose
    }

    fn tools(&self) -> Vec<McpTool> {
        let mut tools = vec![
            McpTool {
                name: "query".to_string(),
                description: "Execute a SQL query and return results. Use for SELECT statements and data retrieval.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "sql": {
                            "type": "string",
                            "description": "SQL query to execute (SELECT, SHOW, DESCRIBE, EXPLAIN)"
                        }
                    },
                    "required": ["sql"]
                }),
            },
            McpTool {
                name: "list_tables".to_string(),
                description: "List all tables and views in the database".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
            McpTool {
                name: "describe_table".to_string(),
                description: "Get the schema/structure of a specific table".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "table_name": {
                            "type": "string",
                            "description": "Name of the table to describe"
                        }
                    },
                    "required": ["table_name"]
                }),
            },
        ];

        // Only add execute tool in fullaccess mode
        if self.config.access_mode == AccessMode::FullAccess {
            tools.push(McpTool {
                name: "execute".to_string(),
                description: "Execute a SQL statement that modifies data (INSERT, UPDATE, DELETE, CREATE, DROP, etc.)".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "sql": {
                            "type": "string",
                            "description": "SQL statement to execute"
                        }
                    },
                    "required": ["sql"]
                }),
            });
        }

        tools
    }

    fn call_tool(&self, name: &str, arguments: &serde_json::Value) -> Result<serde_json::Value> {
        match name {
            "query" => {
                let sql = arguments
                    .get("sql")
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| anyhow!("Missing sql argument"))?;

                match self.execute_query(sql) {
                    Ok(result) => {
                        let result_json = serde_json::to_string_pretty(&result)?;
                        Ok(text_content(&result_json))
                    }
                    Err(e) => Ok(error_content(&e.to_string())),
                }
            }
            "execute" => {
                let sql = arguments
                    .get("sql")
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| anyhow!("Missing sql argument"))?;

                match self.execute_statement(sql) {
                    Ok(result) => {
                        let result_json = serde_json::to_string_pretty(&result)?;
                        Ok(text_content(&result_json))
                    }
                    Err(e) => Ok(error_content(&e.to_string())),
                }
            }
            "list_tables" => match self.list_tables() {
                Ok(tables) => {
                    let result_json = serde_json::to_string_pretty(&tables)?;
                    Ok(text_content(&result_json))
                }
                Err(e) => Ok(error_content(&e.to_string())),
            },
            "describe_table" => {
                let table_name = arguments
                    .get("table_name")
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| anyhow!("Missing table_name argument"))?;

                match self.describe_table(table_name) {
                    Ok(columns) => {
                        let result_json = serde_json::to_string_pretty(&columns)?;
                        Ok(text_content(&result_json))
                    }
                    Err(e) => Ok(error_content(&e.to_string())),
                }
            }
            _ => Ok(error_content(&format!("Unknown tool: {}", name))),
        }
    }
}

/// Create and run the SQL MCP server
pub fn run_sql_server(config: SqlServerConfig) -> Result<()> {
    if config.verbose {
        eprintln!("[mcpz] SQL server configuration:");
        eprintln!("[mcpz]   Database: {}", config.database_type());
        eprintln!("[mcpz]   Access mode: {:?}", config.access_mode);
        eprintln!("[mcpz]   Timeout: {:?}", config.timeout);
    }

    // Create tokio runtime for async SQL operations
    let runtime = tokio::runtime::Runtime::new()?;

    // Install any driver support
    sqlx::any::install_default_drivers();

    // Connect to database
    let pool = runtime.block_on(async {
        sqlx::any::AnyPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(config.timeout)
            .connect(&config.connection_string)
            .await
    })?;

    if config.verbose {
        eprintln!("[mcpz] Connected to database successfully");
    }

    let server = SqlServer::new(config, pool, runtime);
    server.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_config_database_type() {
        let config = SqlServerConfig::new(
            "postgres://localhost/test".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );
        assert_eq!(config.database_type(), "PostgreSQL");

        let config = SqlServerConfig::new(
            "postgresql://localhost/test".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );
        assert_eq!(config.database_type(), "PostgreSQL");

        let config = SqlServerConfig::new(
            "mysql://localhost/test".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );
        assert_eq!(config.database_type(), "MySQL");

        let config = SqlServerConfig::new(
            "sqlite:///tmp/test.db".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );
        assert_eq!(config.database_type(), "SQLite");

        let config = SqlServerConfig::new(
            "sqlite::memory:".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );
        assert_eq!(config.database_type(), "SQLite");
    }

    #[test]
    fn test_sql_config_is_statement_allowed_readonly() {
        let config = SqlServerConfig::new(
            "postgres://localhost/test".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );

        // Allowed in readonly
        assert!(config.is_statement_allowed("SELECT * FROM users"));
        assert!(config.is_statement_allowed("select * from users"));
        assert!(config.is_statement_allowed("  SELECT * FROM users"));
        assert!(config.is_statement_allowed("WITH cte AS (SELECT 1) SELECT * FROM cte"));
        assert!(config.is_statement_allowed("EXPLAIN SELECT * FROM users"));
        assert!(config.is_statement_allowed("SHOW TABLES"));
        assert!(config.is_statement_allowed("DESCRIBE users"));
        assert!(config.is_statement_allowed("DESC users"));
        assert!(config.is_statement_allowed("PRAGMA table_info(users)"));

        // Not allowed in readonly
        assert!(!config.is_statement_allowed("INSERT INTO users VALUES (1)"));
        assert!(!config.is_statement_allowed("UPDATE users SET name = 'test'"));
        assert!(!config.is_statement_allowed("DELETE FROM users"));
        assert!(!config.is_statement_allowed("DROP TABLE users"));
        assert!(!config.is_statement_allowed("CREATE TABLE test (id INT)"));
        assert!(!config.is_statement_allowed("ALTER TABLE users ADD COLUMN test INT"));
        assert!(!config.is_statement_allowed("TRUNCATE users"));
    }

    #[test]
    fn test_sql_config_is_statement_allowed_fullaccess() {
        let config = SqlServerConfig::new(
            "postgres://localhost/test".to_string(),
            AccessMode::FullAccess,
            30,
            false,
        );

        // All allowed in fullaccess
        assert!(config.is_statement_allowed("SELECT * FROM users"));
        assert!(config.is_statement_allowed("INSERT INTO users VALUES (1)"));
        assert!(config.is_statement_allowed("UPDATE users SET name = 'test'"));
        assert!(config.is_statement_allowed("DELETE FROM users"));
        assert!(config.is_statement_allowed("DROP TABLE users"));
        assert!(config.is_statement_allowed("CREATE TABLE test (id INT)"));
    }

    #[test]
    fn test_sql_server_tools_readonly() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        sqlx::any::install_default_drivers();

        let pool = runtime.block_on(async {
            sqlx::any::AnyPoolOptions::new()
                .connect("sqlite::memory:")
                .await
                .unwrap()
        });

        let config = SqlServerConfig::new(
            "sqlite::memory:".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );

        let server = SqlServer::new(config, pool, runtime);
        let tools = server.tools();

        // Should have query, list_tables, describe_table but NOT execute
        assert_eq!(tools.len(), 3);
        assert!(tools.iter().any(|t| t.name == "query"));
        assert!(tools.iter().any(|t| t.name == "list_tables"));
        assert!(tools.iter().any(|t| t.name == "describe_table"));
        assert!(!tools.iter().any(|t| t.name == "execute"));
    }

    #[test]
    fn test_sql_server_tools_fullaccess() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        sqlx::any::install_default_drivers();

        let pool = runtime.block_on(async {
            sqlx::any::AnyPoolOptions::new()
                .connect("sqlite::memory:")
                .await
                .unwrap()
        });

        let config = SqlServerConfig::new(
            "sqlite::memory:".to_string(),
            AccessMode::FullAccess,
            30,
            false,
        );

        let server = SqlServer::new(config, pool, runtime);
        let tools = server.tools();

        // Should have all 4 tools including execute
        assert_eq!(tools.len(), 4);
        assert!(tools.iter().any(|t| t.name == "query"));
        assert!(tools.iter().any(|t| t.name == "list_tables"));
        assert!(tools.iter().any(|t| t.name == "describe_table"));
        assert!(tools.iter().any(|t| t.name == "execute"));
    }

    #[test]
    fn test_sql_server_query_sqlite() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        sqlx::any::install_default_drivers();

        // Use shared cache for in-memory SQLite to persist across connections
        let pool = runtime.block_on(async {
            let pool = sqlx::any::AnyPoolOptions::new()
                .max_connections(1)  // Single connection ensures same in-memory db
                .connect("sqlite::memory:")
                .await
                .unwrap();

            // Create a test table
            sqlx::query("CREATE TABLE test (id INTEGER PRIMARY KEY, name TEXT)")
                .execute(&pool)
                .await
                .unwrap();

            sqlx::query("INSERT INTO test (id, name) VALUES (1, 'Alice'), (2, 'Bob')")
                .execute(&pool)
                .await
                .unwrap();

            pool
        });

        let config = SqlServerConfig::new(
            "sqlite::memory:".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );

        let server = SqlServer::new(config, pool, runtime);

        // Test query
        let result = server.execute_query("SELECT * FROM test ORDER BY id").unwrap();
        assert_eq!(result.row_count, 2);
        assert_eq!(result.columns, vec!["id", "name"]);
    }

    #[test]
    fn test_sql_server_readonly_blocks_write() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        sqlx::any::install_default_drivers();

        let pool = runtime.block_on(async {
            let pool = sqlx::any::AnyPoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();

            sqlx::query("CREATE TABLE test (id INTEGER PRIMARY KEY)")
                .execute(&pool)
                .await
                .unwrap();

            pool
        });

        let config = SqlServerConfig::new(
            "sqlite::memory:".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );

        let server = SqlServer::new(config, pool, runtime);

        // Try to execute write statement
        let result = server.execute_statement("INSERT INTO test (id) VALUES (1)");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("readonly"));
    }

    #[test]
    fn test_sql_server_list_tables_sqlite() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        sqlx::any::install_default_drivers();

        let pool = runtime.block_on(async {
            let pool = sqlx::any::AnyPoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();

            sqlx::query("CREATE TABLE users (id INTEGER PRIMARY KEY)")
                .execute(&pool)
                .await
                .unwrap();

            sqlx::query("CREATE TABLE posts (id INTEGER PRIMARY KEY)")
                .execute(&pool)
                .await
                .unwrap();

            pool
        });

        let config = SqlServerConfig::new(
            "sqlite::memory:".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );

        let server = SqlServer::new(config, pool, runtime);

        let tables = server.list_tables().unwrap();
        assert_eq!(tables.len(), 2);
        assert!(tables.iter().any(|t| t.name == "users"));
        assert!(tables.iter().any(|t| t.name == "posts"));
    }

    #[test]
    fn test_sql_server_describe_table_sqlite() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        sqlx::any::install_default_drivers();

        let pool = runtime.block_on(async {
            let pool = sqlx::any::AnyPoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();

            sqlx::query("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT)")
                .execute(&pool)
                .await
                .unwrap();

            pool
        });

        let config = SqlServerConfig::new(
            "sqlite::memory:".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );

        let server = SqlServer::new(config, pool, runtime);

        let columns = server.describe_table("users").unwrap();
        assert_eq!(columns.len(), 3);

        let id_col = columns.iter().find(|c| c.name == "id").unwrap();
        assert_eq!(id_col.data_type, "INTEGER");

        let name_col = columns.iter().find(|c| c.name == "name").unwrap();
        assert!(!name_col.is_nullable);

        let email_col = columns.iter().find(|c| c.name == "email").unwrap();
        assert!(email_col.is_nullable);
    }

    #[test]
    fn test_sql_server_call_tool_query() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        sqlx::any::install_default_drivers();

        let pool = runtime.block_on(async {
            let pool = sqlx::any::AnyPoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();

            sqlx::query("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)")
                .execute(&pool)
                .await
                .unwrap();

            sqlx::query("INSERT INTO test VALUES (1, 'hello')")
                .execute(&pool)
                .await
                .unwrap();

            pool
        });

        let config = SqlServerConfig::new(
            "sqlite::memory:".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );

        let server = SqlServer::new(config, pool, runtime);

        let result = server.call_tool("query", &serde_json::json!({"sql": "SELECT * FROM test"})).unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("hello"));
        assert!(text.contains("row_count"));
    }

    #[test]
    fn test_sql_server_initialize() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        sqlx::any::install_default_drivers();

        let pool = runtime.block_on(async {
            sqlx::any::AnyPoolOptions::new()
                .connect("sqlite::memory:")
                .await
                .unwrap()
        });

        let config = SqlServerConfig::new(
            "sqlite::memory:".to_string(),
            AccessMode::ReadOnly,
            30,
            false,
        );

        let server = SqlServer::new(config, pool, runtime);
        let result = server.handle_initialize();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "mcpz-sql");
    }
}
