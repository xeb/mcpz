use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Runtime MCP router tool for running MCP servers via npx, uvx, or cargo
#[derive(Parser)]
#[command(name = "mcpz")]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run an MCP server package
    Run {
        /// Package name (e.g., mcp-server-time, @modelcontextprotocol/server-filesystem)
        package: String,
        /// Automatically pick the first match (no prompt)
        #[arg(long, short = 'f')]
        first: bool,
        /// Additional arguments to pass to the package
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Search for an MCP package in npm/pip/crates.io registries (non-interactive)
    Search {
        /// Package name to search for
        package: String,
    },
    /// Search and pick a package to save to cache
    Pick {
        /// Package name to search for
        package: String,
    },
    /// Clear the package cache
    ClearCache,
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

/// Determines the package type based on the package name
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageType {
    /// Cargo/Rust package (runs with cargo install)
    Cargo,
    /// Python package (runs with uvx)
    Python,
    /// npm package (runs with npx)
    Npm,
}

impl PackageType {
    /// Get the runner command for this package type
    pub fn runner(&self) -> &'static str {
        match self {
            PackageType::Npm => "npx",
            PackageType::Python => "uvx",
            PackageType::Cargo => "cargo",
        }
    }

    /// Get the installer instructions for this package type
    pub fn install_instructions(&self) -> &'static str {
        match self {
            PackageType::Npm => "Install Node.js/npm from https://nodejs.org/ or run: curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash - && sudo apt-get install -y nodejs",
            PackageType::Python => "Install uv by running: curl -LsSf https://astral.sh/uv/install.sh | sh",
            PackageType::Cargo => "Install Rust/Cargo from https://rustup.rs/ or run: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh",
        }
    }

    /// Get display name for this package type
    pub fn display_name(&self) -> &'static str {
        match self {
            PackageType::Npm => "npm",
            PackageType::Python => "PyPI",
            PackageType::Cargo => "crates.io",
        }
    }
}

/// Information about a found package
#[derive(Debug, Clone)]
struct PackageInfo {
    name: String,
    version: String,
    description: String,
    author: String,
    published: String,
    downloads: Option<u64>,
    registry: PackageType,
}

impl PackageInfo {
    fn display(&self, index: usize) {
        println!(
            "{}",
            format!("[{}] {} v{}", index + 1, self.name, self.version)
                .green()
                .bold()
        );
        println!("    Registry:    {}", self.registry.display_name().cyan());
        println!("    Description: {}", self.description);
        println!("    Author:      {}", self.author);
        println!("    Published:   {}", self.published);
        if let Some(dl) = self.downloads {
            println!("    Downloads:   {}", format_downloads(dl).yellow());
        }
        println!();
    }
}

/// Format download count with K/M suffix
fn format_downloads(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// Sort packages by download count (most popular first)
fn sort_by_popularity(packages: &mut [PackageInfo]) {
    packages.sort_by(|a, b| {
        match (b.downloads, a.downloads) {
            (Some(b_dl), Some(a_dl)) => b_dl.cmp(&a_dl),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
}

/// Package cache stored in ~/.cache/mcpz/package_mapping.toml
#[derive(Debug, Default, Serialize, Deserialize)]
struct PackageCache {
    /// Maps search term -> (actual package name, package type)
    packages: HashMap<String, (String, PackageType)>,
}

impl PackageCache {
    fn cache_path() -> Result<PathBuf> {
        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| anyhow!("Could not determine cache directory"))?
            .join("mcpz");
        Ok(cache_dir.join("package_mapping.toml"))
    }

    fn load() -> Result<Self> {
        let path = Self::cache_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path).context("Failed to read cache file")?;
        toml::from_str(&content).context("Failed to parse cache file")
    }

    fn save(&self) -> Result<()> {
        let path = Self::cache_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create cache directory")?;
        }

        let content = toml::to_string_pretty(self).context("Failed to serialize cache")?;
        fs::write(&path, content).context("Failed to write cache file")?;
        Ok(())
    }

    fn get(&self, search_term: &str) -> Option<(String, PackageType)> {
        self.packages.get(search_term).cloned()
    }

    fn set(&mut self, search_term: String, package_name: String, pkg_type: PackageType) {
        self.packages.insert(search_term, (package_name, pkg_type));
    }

    fn clear() -> Result<()> {
        let path = Self::cache_path()?;
        if path.exists() {
            fs::remove_file(&path).context("Failed to remove cache file")?;
        }
        Ok(())
    }
}

/// Check if a command exists on the system
pub fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Get npm download count for a package
fn get_npm_downloads(client: &reqwest::blocking::Client, package: &str) -> Option<u64> {
    let url = format!(
        "https://api.npmjs.org/downloads/point/last-month/{}",
        package
    );
    let resp = client.get(&url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let data: serde_json::Value = resp.json().ok()?;
    data.get("downloads").and_then(|v| v.as_u64())
}

/// Search npm registry and return matching packages
fn search_npm(query: &str) -> Vec<PackageInfo> {
    if !command_exists("npm") {
        return vec![];
    }

    let output = Command::new("npm")
        .args(["search", "--json", query])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };

    let results: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok();

    let mut packages = vec![];
    if let Some(arr) = results.as_array() {
        for item in arr.iter().take(10) {
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let version = item.get("version").and_then(|v| v.as_str()).unwrap_or("?");
            let description = item
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("No description");
            let author = item
                .get("publisher")
                .and_then(|p| p.get("username"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown");
            let date = item.get("date").and_then(|v| v.as_str()).unwrap_or("Unknown");
            let published = date.split('T').next().unwrap_or(date).to_string();

            // Get download count
            let downloads = client
                .as_ref()
                .and_then(|c| get_npm_downloads(c, name));

            if !name.is_empty() {
                packages.push(PackageInfo {
                    name: name.to_string(),
                    version: version.to_string(),
                    description: description.to_string(),
                    author: author.to_string(),
                    published,
                    downloads,
                    registry: PackageType::Npm,
                });
            }
        }
    }

    packages
}

/// Get PyPI download count for a package (last month)
fn get_pypi_downloads(client: &reqwest::blocking::Client, package: &str) -> Option<u64> {
    let url = format!("https://pypistats.org/api/packages/{}/recent", package);
    let resp = client.get(&url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let data: serde_json::Value = resp.json().ok()?;
    data.get("data")
        .and_then(|d| d.get("last_month"))
        .and_then(|v| v.as_u64())
}

/// Search PyPI registry and return matching packages
fn search_pypi(query: &str) -> Vec<PackageInfo> {
    // PyPI doesn't have a search API, so we'll check if the exact package exists
    // and also try common variations
    let variations = vec![
        query.to_string(),
        query.replace('-', "_"),
        query.replace('_', "-"),
    ];

    let mut packages = vec![];
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return packages,
    };

    for pkg_name in variations {
        let url = format!("https://pypi.org/pypi/{}/json", pkg_name);
        let resp = match client.get(&url).send() {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };

        let data: serde_json::Value = match resp.json() {
            Ok(v) => v,
            Err(_) => continue,
        };

        let info = match data.get("info") {
            Some(i) => i,
            None => continue,
        };

        let name = info.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let version = info.get("version").and_then(|v| v.as_str()).unwrap_or("?");
        let description = info
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("No description");
        let author = info
            .get("author")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                info.get("author_email")
                    .and_then(|v| v.as_str())
                    .map(|s| s.split('<').next().unwrap_or(s).trim())
            })
            .unwrap_or("Unknown");

        // Get upload time from releases
        let published = data
            .get("urls")
            .and_then(|u| u.as_array())
            .and_then(|arr| arr.first())
            .and_then(|u| u.get("upload_time"))
            .and_then(|v| v.as_str())
            .map(|s| s.split('T').next().unwrap_or(s))
            .unwrap_or("Unknown")
            .to_string();

        // Get download count
        let downloads = get_pypi_downloads(&client, name);

        if !name.is_empty() && !packages.iter().any(|p: &PackageInfo| p.name == name) {
            packages.push(PackageInfo {
                name: name.to_string(),
                version: version.to_string(),
                description: description.to_string(),
                author: author.to_string(),
                published,
                downloads,
                registry: PackageType::Python,
            });
        }
    }

    packages
}

/// Search crates.io registry and return matching packages (using API for full details)
fn search_cargo(query: &str) -> Vec<PackageInfo> {
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("mcpz")
        .build()
    {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    // Use crates.io API for search
    let url = format!(
        "https://crates.io/api/v1/crates?q={}&per_page=10",
        urlencoding::encode(query)
    );

    let resp = match client.get(&url).send() {
        Ok(r) if r.status().is_success() => r,
        _ => return vec![],
    };

    let data: serde_json::Value = match resp.json() {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let mut packages = vec![];

    if let Some(crates) = data.get("crates").and_then(|c| c.as_array()) {
        for item in crates.iter().take(10) {
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let version = item
                .get("newest_version")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let description = item
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("No description");
            let downloads = item.get("downloads").and_then(|v| v.as_u64());
            let updated = item
                .get("updated_at")
                .and_then(|v| v.as_str())
                .map(|s| s.split('T').next().unwrap_or(s))
                .unwrap_or("Unknown")
                .to_string();

            if !name.is_empty() {
                packages.push(PackageInfo {
                    name: name.to_string(),
                    version: version.to_string(),
                    description: description.to_string(),
                    author: "See crates.io".to_string(),
                    published: updated,
                    downloads,
                    registry: PackageType::Cargo,
                });
            }
        }
    }

    packages
}

/// Search all registries and let user pick a package
fn search_and_select(query: &str) -> Result<Option<(String, PackageType)>> {
    println!(
        "{}",
        format!("Searching for '{}' across all registries...", query).cyan()
    );
    println!();

    let mut all_packages = vec![];

    // Search cargo first
    print!("  Searching crates.io... ");
    std::io::stdout().flush()?;
    let cargo_results = search_cargo(query);
    println!("{} found", cargo_results.len());
    all_packages.extend(cargo_results);

    // Search PyPI
    print!("  Searching PyPI... ");
    std::io::stdout().flush()?;
    let pypi_results = search_pypi(query);
    println!("{} found", pypi_results.len());
    all_packages.extend(pypi_results);

    // Search npm
    print!("  Searching npm... ");
    std::io::stdout().flush()?;
    let npm_results = search_npm(query);
    println!("{} found", npm_results.len());
    all_packages.extend(npm_results);

    println!();

    if all_packages.is_empty() {
        println!(
            "{}",
            format!("No packages found matching '{}'", query).red()
        );
        return Ok(None);
    }

    // Sort by popularity (most downloads first)
    sort_by_popularity(&mut all_packages);

    // Display all packages
    println!(
        "{}",
        format!("Found {} packages (sorted by popularity):", all_packages.len())
            .green()
            .bold()
    );
    println!();

    for (i, pkg) in all_packages.iter().enumerate() {
        pkg.display(i);
    }

    // Let user select
    print!(
        "{}",
        "Select a package (1-{}) or 'q' to quit: "
            .replace("{}", &all_packages.len().to_string())
            .yellow()
    );
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.eq_ignore_ascii_case("q") {
        return Ok(None);
    }

    let selection: usize = input.parse().context("Invalid selection")?;
    if selection < 1 || selection > all_packages.len() {
        return Err(anyhow!("Selection out of range"));
    }

    let selected = &all_packages[selection - 1];
    println!();
    println!(
        "{}",
        format!(
            "Selected: {} ({}) from {}",
            selected.name,
            selected.version,
            selected.registry.display_name()
        )
        .green()
    );

    Ok(Some((selected.name.clone(), selected.registry)))
}

/// Discover package type by searching registries
/// If multiple exact matches found, let user pick (unless pick_first is true)
fn discover_package_type(package: &str, pick_first: bool) -> Result<(String, PackageType)> {
    // npm scoped packages start with @ - skip other checks
    if package.starts_with('@') {
        println!("{}", format!("Checking npm for '{}'...", package).cyan());
        let results = search_npm(package);
        if results.iter().any(|p| p.name == package) {
            println!("{}", format!("✓ Found in npm: {}", package).green());
            return Ok((package.to_string(), PackageType::Npm));
        }
        return Err(anyhow!("Package '{}' not found in npm", package));
    }

    // Search all registries to find exact matches
    println!(
        "{}",
        format!("Searching for '{}' across registries...", package).cyan()
    );

    let mut exact_matches: Vec<PackageInfo> = vec![];

    // Check cargo
    let cargo_results = search_cargo(package);
    if let Some(pkg) = cargo_results.iter().find(|p| p.name == package) {
        exact_matches.push(pkg.clone());
    }

    // Check PyPI
    let pypi_results = search_pypi(package);
    if let Some(pkg) = pypi_results.iter().find(|p| {
        p.name == package
            || p.name == package.replace('-', "_")
            || p.name == package.replace('_', "-")
    }) {
        exact_matches.push(pkg.clone());
    }

    // Check npm
    let npm_results = search_npm(package);
    if let Some(pkg) = npm_results.iter().find(|p| p.name == package) {
        exact_matches.push(pkg.clone());
    }

    // Sort by popularity (most downloads first)
    sort_by_popularity(&mut exact_matches);

    match exact_matches.len() {
        0 => Err(anyhow!(
            "Package '{}' not found in any registry (crates.io, PyPI, npm)",
            package
        )),
        1 => {
            let pkg = &exact_matches[0];
            println!(
                "{}",
                format!("✓ Found in {}: {}", pkg.registry.display_name(), pkg.name).green()
            );
            Ok((pkg.name.clone(), pkg.registry))
        }
        _ => {
            // Multiple matches found - already sorted by popularity
            if pick_first {
                // Auto-pick most popular (first after sort)
                let pkg = &exact_matches[0];
                println!(
                    "{}",
                    format!(
                        "✓ Multiple matches found, auto-selecting first: {} ({})",
                        pkg.name,
                        pkg.registry.display_name()
                    )
                    .green()
                );
                return Ok((pkg.name.clone(), pkg.registry));
            }

            // Let user pick
            println!();
            println!(
                "{}",
                format!(
                    "Found '{}' in {} registries. Please choose:",
                    package,
                    exact_matches.len()
                )
                .yellow()
                .bold()
            );
            println!();

            for (i, pkg) in exact_matches.iter().enumerate() {
                pkg.display(i);
            }

            print!(
                "{}",
                format!("Select a package (1-{}): ", exact_matches.len()).yellow()
            );
            std::io::stdout().flush()?;

            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            let selection: usize = input.trim().parse().context("Invalid selection")?;

            if selection < 1 || selection > exact_matches.len() {
                return Err(anyhow!("Selection out of range"));
            }

            let selected = &exact_matches[selection - 1];
            println!();
            println!(
                "{}",
                format!(
                    "Selected: {} from {}",
                    selected.name,
                    selected.registry.display_name()
                )
                .green()
            );

            Ok((selected.name.clone(), selected.registry))
        }
    }
}

/// Get package type, using cache if available
fn get_package_type(package: &str, pick_first: bool) -> Result<(String, PackageType)> {
    let mut cache = PackageCache::load().unwrap_or_default();

    // Check cache first
    if let Some((pkg_name, pkg_type)) = cache.get(package) {
        println!(
            "{}",
            format!(
                "Using cached runtime for '{}': {} ({})",
                package,
                pkg_name,
                pkg_type.display_name()
            )
            .cyan()
        );
        return Ok((pkg_name, pkg_type));
    }

    // Discover package type
    let (pkg_name, pkg_type) = discover_package_type(package, pick_first)?;

    // Save to cache
    cache.set(package.to_string(), pkg_name.clone(), pkg_type);
    if let Err(e) = cache.save() {
        eprintln!(
            "{}",
            format!("Warning: Failed to save cache: {}", e).yellow()
        );
    }

    Ok((pkg_name, pkg_type))
}

/// Install uv if not present
fn install_uv() -> Result<()> {
    println!(
        "{}",
        "uv/uvx not found. Would you like to install it? [y/N]".yellow()
    );

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    if input.trim().to_lowercase() != "y" {
        return Err(anyhow!("Installation cancelled by user"));
    }

    println!("{}", "Installing uv...".cyan());

    let status = Command::new("sh")
        .args(["-c", "curl -LsSf https://astral.sh/uv/install.sh | sh"])
        .status()
        .context("Failed to install uv")?;

    if !status.success() {
        return Err(anyhow!("Failed to install uv"));
    }

    println!("{}", "✓ uv installed successfully".green());
    Ok(())
}

/// Run an MCP server package
fn run_package(package: &str, args: &[String], pick_first: bool) -> Result<()> {
    let (pkg_name, pkg_type) = get_package_type(package, pick_first)?;
    let runner = pkg_type.runner();

    // Check if runner exists
    if !command_exists(runner) {
        match pkg_type {
            PackageType::Python => {
                install_uv()?;
                if !command_exists(runner) {
                    return Err(anyhow!(
                        "{} still not found after installation. You may need to restart your shell or add it to PATH.",
                        runner
                    ));
                }
            }
            PackageType::Npm | PackageType::Cargo => {
                return Err(anyhow!(
                    "{} not found. {}",
                    runner,
                    pkg_type.install_instructions()
                ));
            }
        }
    }

    // Handle Cargo packages differently - install first, then run the binary
    if pkg_type == PackageType::Cargo {
        return run_cargo_package(&pkg_name, args);
    }

    println!(
        "{}",
        format!(
            "Running: {} {} {} {}",
            runner,
            if pkg_type == PackageType::Npm {
                "-y"
            } else {
                ""
            },
            pkg_name,
            args.join(" ")
        )
        .trim()
        .cyan()
    );

    let mut cmd = Command::new(runner);

    if pkg_type == PackageType::Npm {
        cmd.arg("-y");
    }

    cmd.arg(&pkg_name);
    cmd.args(args);

    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context(format!("Failed to spawn {}", runner))?;

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        std::thread::spawn(move || {
            for line in reader.lines() {
                if let Ok(line) = line {
                    println!("{}", line);
                }
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let reader = BufReader::new(stderr);
        std::thread::spawn(move || {
            for line in reader.lines() {
                if let Ok(line) = line {
                    eprintln!("{}", line.red());
                }
            }
        });
    }

    let status = child.wait().context("Failed to wait for child process")?;

    if !status.success() {
        return Err(anyhow!("Process exited with status: {}", status));
    }

    Ok(())
}

/// Run a Cargo package by installing it first, then running the binary
fn run_cargo_package(package: &str, args: &[String]) -> Result<()> {
    if !command_exists(package) {
        println!(
            "{}",
            format!("Installing cargo package '{}'...", package).cyan()
        );

        let status = Command::new("cargo")
            .args(["install", package])
            .status()
            .context("Failed to run cargo install")?;

        if !status.success() {
            return Err(anyhow!("Failed to install cargo package: {}", package));
        }

        println!("{}", format!("✓ Installed {}", package).green());
    }

    println!(
        "{}",
        format!("Running: {} {}", package, args.join(" ")).cyan()
    );

    let mut cmd = Command::new(package);
    cmd.args(args);

    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context(format!("Failed to spawn {}", package))?;

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        std::thread::spawn(move || {
            for line in reader.lines() {
                if let Ok(line) = line {
                    println!("{}", line);
                }
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let reader = BufReader::new(stderr);
        std::thread::spawn(move || {
            for line in reader.lines() {
                if let Ok(line) = line {
                    eprintln!("{}", line.red());
                }
            }
        });
    }

    let status = child.wait().context("Failed to wait for child process")?;

    if !status.success() {
        return Err(anyhow!("Process exited with status: {}", status));
    }

    Ok(())
}

/// Non-interactive search - just display results
fn search_package(query: &str) -> Result<()> {
    println!(
        "{}",
        format!("Searching for '{}' across all registries...", query).cyan()
    );
    println!();

    let mut all_packages = vec![];

    // Search cargo first
    print!("  Searching crates.io... ");
    std::io::stdout().flush()?;
    let cargo_results = search_cargo(query);
    println!("{} found", cargo_results.len());
    all_packages.extend(cargo_results);

    // Search PyPI
    print!("  Searching PyPI... ");
    std::io::stdout().flush()?;
    let pypi_results = search_pypi(query);
    println!("{} found", pypi_results.len());
    all_packages.extend(pypi_results);

    // Search npm
    print!("  Searching npm... ");
    std::io::stdout().flush()?;
    let npm_results = search_npm(query);
    println!("{} found", npm_results.len());
    all_packages.extend(npm_results);

    println!();

    if all_packages.is_empty() {
        println!(
            "{}",
            format!("No packages found matching '{}'", query).red()
        );
        return Ok(());
    }

    // Sort by popularity (most downloads first)
    sort_by_popularity(&mut all_packages);

    // Display all packages
    println!(
        "{}",
        format!("Found {} packages (sorted by popularity):", all_packages.len())
            .green()
            .bold()
    );
    println!();

    for (i, pkg) in all_packages.iter().enumerate() {
        pkg.display(i);
    }

    Ok(())
}

/// Interactive pick - show results and let user pick one to save to cache
fn pick_package(query: &str) -> Result<()> {
    let selection = search_and_select(query)?;

    if let Some((pkg_name, pkg_type)) = selection {
        // Ask if user wants to save to cache
        print!(
            "{}",
            format!(
                "Save '{}' -> {} ({}) to cache for future runs? [Y/n]: ",
                query,
                pkg_name,
                pkg_type.display_name()
            )
            .yellow()
        );
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() || input.eq_ignore_ascii_case("y") {
            let mut cache = PackageCache::load().unwrap_or_default();
            cache.set(query.to_string(), pkg_name.clone(), pkg_type);
            cache.save()?;
            println!("{}", "✓ Saved to cache".green());
        }

        // Ask if user wants to run it now
        print!("{}", "Run it now? [y/N]: ".yellow());
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.eq_ignore_ascii_case("y") {
            run_package(&pkg_name, &[], false)?;
        }
    }

    Ok(())
}

// ============================================================================
// MCP Shell Server Implementation
// ============================================================================

/// Configuration for the shell server
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

/// JSON-RPC request structure
#[derive(Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

/// JSON-RPC response structure
#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

/// JSON-RPC error structure
#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// Command execution result
#[derive(Serialize)]
struct ShellCommandResult {
    command: String,
    output: String,
    return_code: i32,
}

/// Execute a shell command with the given config
fn execute_shell_command(command: &str, config: &ShellServerConfig) -> ShellCommandResult {
    // Check sandboxing rules
    if !config.is_command_allowed(command) {
        if config.verbose {
            eprintln!("[mcpz] Command denied by security policy: {}", command);
        }
        return ShellCommandResult {
            command: command.to_string(),
            output: "Command denied by security policy".to_string(),
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

            let return_code = output.status.code().unwrap_or(-1);

            if config.verbose {
                eprintln!("[mcpz] Exit code: {}", return_code);
            }

            ShellCommandResult {
                command: command.to_string(),
                output: combined,
                return_code,
            }
        }
        Err(e) => {
            if config.verbose {
                eprintln!("[mcpz] Error: {}", e);
            }
            ShellCommandResult {
                command: command.to_string(),
                output: format!("Failed to execute: {}", e),
                return_code: -1,
            }
        }
    }
}

/// Handle the initialize request
fn handle_initialize() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "mcpz-shell",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

/// Handle the tools/list request
fn handle_tools_list() -> serde_json::Value {
    serde_json::json!({
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
    })
}

/// Handle the tools/call request
fn handle_tools_call(params: &serde_json::Value, config: &ShellServerConfig) -> Result<serde_json::Value> {
    let name = params.get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing tool name"))?;

    if name != "execute_command" {
        return Err(anyhow!("Unknown tool: {}", name));
    }

    let command = params.get("arguments")
        .and_then(|a| a.get("command"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow!("Missing command argument"))?;

    let result = execute_shell_command(command, config);

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&result)?
        }]
    }))
}

/// Handle a JSON-RPC request
fn handle_mcp_request(req: JsonRpcRequest, config: &ShellServerConfig) -> Option<JsonRpcResponse> {
    match req.method.as_str() {
        "initialize" => {
            Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(handle_initialize()),
                error: None,
            })
        }
        "initialized" => {
            // Notification - no response
            None
        }
        "notifications/initialized" => {
            // Alternative notification format - no response
            None
        }
        "tools/list" => {
            Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(handle_tools_list()),
                error: None,
            })
        }
        "tools/call" => {
            match handle_tools_call(&req.params, config) {
                Ok(result) => Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: Some(result),
                    error: None,
                }),
                Err(e) => Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32603,
                        message: e.to_string(),
                    }),
                }),
            }
        }
        _ => {
            Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("Method not found: {}", req.method),
                }),
            })
        }
    }
}

/// Run the MCP shell server
fn run_shell_server(config: ShellServerConfig) -> Result<()> {
    if config.verbose {
        eprintln!("[mcpz] Shell server started");
        eprintln!("[mcpz] Working dir: {:?}", config.working_dir);
        eprintln!("[mcpz] Shell: {}", config.shell);
        eprintln!("[mcpz] Timeout: {:?}", config.timeout);
        if !config.allow_patterns.is_empty() {
            eprintln!("[mcpz] Allow patterns: {:?}", config.allow_patterns);
        }
        if !config.deny_patterns.is_empty() {
            eprintln!("[mcpz] Deny patterns: {:?}", config.deny_patterns);
        }
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                if config.verbose {
                    eprintln!("[mcpz] Error reading stdin: {}", e);
                }
                break;
            }
        };

        if line.is_empty() {
            continue;
        }

        if config.verbose {
            eprintln!("[mcpz] Received: {}", line);
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                if config.verbose {
                    eprintln!("[mcpz] Parse error: {}", e);
                }
                let error_response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("Parse error: {}", e),
                    }),
                };
                let response_json = serde_json::to_string(&error_response)?;
                writeln!(stdout, "{}", response_json)?;
                stdout.flush()?;
                continue;
            }
        };

        if let Some(response) = handle_mcp_request(request, &config) {
            let response_json = serde_json::to_string(&response)?;
            if config.verbose {
                eprintln!("[mcpz] Sending: {}", response_json);
            }
            writeln!(stdout, "{}", response_json)?;
            stdout.flush()?;
        }
    }

    if config.verbose {
        eprintln!("[mcpz] Shell server stopped");
    }

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { package, first, args } => run_package(&package, &args, first),
        Commands::Search { package } => search_package(&package),
        Commands::Pick { package } => pick_package(&package),
        Commands::ClearCache => {
            PackageCache::clear()?;
            println!("{}", "✓ Cache cleared".green());
            Ok(())
        }
        Commands::Server { server_type } => {
            match server_type {
                ServerType::Shell {
                    working_dir,
                    timeout,
                    shell,
                    allow,
                    deny,
                    no_stderr,
                    verbose,
                } => {
                    let config = ShellServerConfig::from_args(
                        working_dir,
                        timeout,
                        shell,
                        allow,
                        deny,
                        no_stderr,
                        verbose,
                    );
                    run_shell_server(config)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_type_runner() {
        assert_eq!(PackageType::Npm.runner(), "npx");
        assert_eq!(PackageType::Python.runner(), "uvx");
        assert_eq!(PackageType::Cargo.runner(), "cargo");
    }

    #[test]
    fn test_package_type_display_name() {
        assert_eq!(PackageType::Npm.display_name(), "npm");
        assert_eq!(PackageType::Python.display_name(), "PyPI");
        assert_eq!(PackageType::Cargo.display_name(), "crates.io");
    }

    #[test]
    fn test_command_exists_which() {
        assert!(command_exists("which"));
    }

    #[test]
    fn test_command_exists_nonexistent() {
        assert!(!command_exists("this-command-definitely-does-not-exist-12345"));
    }

    #[test]
    fn test_cli_parse_run() {
        let cli = Cli::parse_from(["mcpz", "run", "@modelcontextprotocol/server-filesystem", "."]);
        match cli.command {
            Commands::Run { package, first, args } => {
                assert_eq!(package, "@modelcontextprotocol/server-filesystem");
                assert!(!first);
                assert_eq!(args, vec!["."]);
            }
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_cli_parse_run_no_args() {
        let cli = Cli::parse_from(["mcpz", "run", "mcp-server-time"]);
        match cli.command {
            Commands::Run { package, first, args } => {
                assert_eq!(package, "mcp-server-time");
                assert!(!first);
                assert!(args.is_empty());
            }
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_cli_parse_run_first() {
        let cli = Cli::parse_from(["mcpz", "run", "--first", "mcp-server-time"]);
        match cli.command {
            Commands::Run { package, first, args } => {
                assert_eq!(package, "mcp-server-time");
                assert!(first);
                assert!(args.is_empty());
            }
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_cli_parse_search() {
        let cli = Cli::parse_from(["mcpz", "search", "mcp-server-time"]);
        match cli.command {
            Commands::Search { package } => {
                assert_eq!(package, "mcp-server-time");
            }
            _ => panic!("Expected Search command"),
        }
    }

    #[test]
    fn test_cli_parse_pick() {
        let cli = Cli::parse_from(["mcpz", "pick", "mcp-server-time"]);
        match cli.command {
            Commands::Pick { package } => {
                assert_eq!(package, "mcp-server-time");
            }
            _ => panic!("Expected Pick command"),
        }
    }

    #[test]
    fn test_cli_parse_clear_cache() {
        let cli = Cli::parse_from(["mcpz", "clear-cache"]);
        assert!(matches!(cli.command, Commands::ClearCache));
    }

    #[test]
    fn test_package_type_install_instructions() {
        let npm_instructions = PackageType::Npm.install_instructions();
        assert!(npm_instructions.contains("nodejs") || npm_instructions.contains("Node"));

        let python_instructions = PackageType::Python.install_instructions();
        assert!(python_instructions.contains("astral.sh"));

        let cargo_instructions = PackageType::Cargo.install_instructions();
        assert!(cargo_instructions.contains("rustup") || cargo_instructions.contains("Rust"));
    }

    #[test]
    fn test_cache_serialization() {
        let mut cache = PackageCache::default();
        cache.set(
            "test-search".to_string(),
            "actual-package".to_string(),
            PackageType::Python,
        );
        cache.set(
            "another".to_string(),
            "another-pkg".to_string(),
            PackageType::Npm,
        );

        let serialized = toml::to_string(&cache).unwrap();
        let deserialized: PackageCache = toml::from_str(&serialized).unwrap();

        assert_eq!(
            deserialized.get("test-search"),
            Some(("actual-package".to_string(), PackageType::Python))
        );
        assert_eq!(
            deserialized.get("another"),
            Some(("another-pkg".to_string(), PackageType::Npm))
        );
    }

    // Shell server tests

    #[test]
    fn test_cli_parse_server_shell() {
        let cli = Cli::parse_from(["mcpz", "server", "shell"]);
        match cli.command {
            Commands::Server { server_type } => {
                match server_type {
                    ServerType::Shell { working_dir, timeout, shell, allow, deny, no_stderr, verbose } => {
                        assert!(working_dir.is_none());
                        assert_eq!(timeout, 30);
                        assert_eq!(shell, "/bin/sh");
                        assert!(allow.is_none());
                        assert!(deny.is_none());
                        assert!(!no_stderr);
                        assert!(!verbose);
                    }
                }
            }
            _ => panic!("Expected Server command"),
        }
    }

    #[test]
    fn test_cli_parse_server_shell_with_options() {
        let cli = Cli::parse_from([
            "mcpz", "server", "shell",
            "--working-dir", "/tmp",
            "--timeout", "60",
            "--shell", "/bin/bash",
            "--allow", "ls*,cat*",
            "--deny", "rm*,sudo*",
            "--no-stderr",
            "--verbose",
        ]);
        match cli.command {
            Commands::Server { server_type } => {
                match server_type {
                    ServerType::Shell { working_dir, timeout, shell, allow, deny, no_stderr, verbose } => {
                        assert_eq!(working_dir, Some(PathBuf::from("/tmp")));
                        assert_eq!(timeout, 60);
                        assert_eq!(shell, "/bin/bash");
                        assert_eq!(allow, Some("ls*,cat*".to_string()));
                        assert_eq!(deny, Some("rm*,sudo*".to_string()));
                        assert!(no_stderr);
                        assert!(verbose);
                    }
                }
            }
            _ => panic!("Expected Server command"),
        }
    }

    #[test]
    fn test_shell_config_pattern_matching() {
        // Test wildcard matching
        assert!(ShellServerConfig::matches_pattern("ls -la", "ls*"));
        assert!(ShellServerConfig::matches_pattern("ls", "ls*"));
        assert!(ShellServerConfig::matches_pattern("lsblk", "ls*"));
        assert!(!ShellServerConfig::matches_pattern("cat file", "ls*"));

        // Test exact matching
        assert!(ShellServerConfig::matches_pattern("ls -la", "ls"));
        assert!(!ShellServerConfig::matches_pattern("lsblk", "ls"));
    }

    #[test]
    fn test_shell_config_is_command_allowed() {
        // No restrictions - allow all
        let config = ShellServerConfig::from_args(None, 30, "/bin/sh".to_string(), None, None, false, false);
        assert!(config.is_command_allowed("ls -la"));
        assert!(config.is_command_allowed("rm -rf /"));

        // Only allow list
        let config = ShellServerConfig::from_args(None, 30, "/bin/sh".to_string(), Some("ls*,cat*".to_string()), None, false, false);
        assert!(config.is_command_allowed("ls -la"));
        assert!(config.is_command_allowed("cat file"));
        assert!(!config.is_command_allowed("rm file"));

        // Only deny list
        let config = ShellServerConfig::from_args(None, 30, "/bin/sh".to_string(), None, Some("rm*,sudo*".to_string()), false, false);
        assert!(config.is_command_allowed("ls -la"));
        assert!(!config.is_command_allowed("rm file"));
        assert!(!config.is_command_allowed("sudo ls"));

        // Both allow and deny - deny takes precedence
        let config = ShellServerConfig::from_args(None, 30, "/bin/sh".to_string(), Some("*".to_string()), Some("rm*".to_string()), false, false);
        assert!(!config.is_command_allowed("rm file"));
    }

    #[test]
    fn test_execute_shell_command() {
        let config = ShellServerConfig::from_args(None, 30, "/bin/sh".to_string(), None, None, false, false);
        let result = execute_shell_command("echo hello", &config);
        assert_eq!(result.command, "echo hello");
        assert!(result.output.contains("hello"));
        assert_eq!(result.return_code, 0);
    }

    #[test]
    fn test_execute_shell_command_denied() {
        let config = ShellServerConfig::from_args(None, 30, "/bin/sh".to_string(), Some("ls*".to_string()), None, false, false);
        let result = execute_shell_command("rm file", &config);
        assert_eq!(result.return_code, -1);
        assert!(result.output.contains("denied"));
    }

    #[test]
    fn test_handle_initialize() {
        let result = handle_initialize();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "mcpz-shell");
    }

    #[test]
    fn test_handle_tools_list() {
        let result = handle_tools_list();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "execute_command");
    }

    #[test]
    fn test_handle_tools_call() {
        let config = ShellServerConfig::from_args(None, 30, "/bin/sh".to_string(), None, None, false, false);
        let params = serde_json::json!({
            "name": "execute_command",
            "arguments": {
                "command": "echo test"
            }
        });
        let result = handle_tools_call(&params, &config).unwrap();
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        let text = content[0]["text"].as_str().unwrap();
        assert!(text.contains("test"));
    }

    #[test]
    fn test_json_rpc_request_parsing() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(serde_json::json!(1)));
        assert_eq!(req.method, "initialize");
    }

    #[test]
    fn test_json_rpc_response_serialization() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            result: Some(serde_json::json!({"test": true})),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"test\":true"));
        assert!(!json.contains("error"));
    }
}
