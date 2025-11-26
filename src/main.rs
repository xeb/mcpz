use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
}
