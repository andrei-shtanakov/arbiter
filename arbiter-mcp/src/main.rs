//! Arbiter MCP Server — Coding Agent Policy Engine.
//!
//! Binary that implements the MCP server over stdio.
//! Handles JSON-RPC 2.0 dispatch, lifecycle management,
//! and tool execution.

use std::path::PathBuf;
use std::process;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use arbiter_core::policy::decision_tree::DecisionTree;
use arbiter_mcp::{agents, config, db, server};
use tracing::info;

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

/// Parsed command-line arguments.
struct CliArgs {
    /// Path to decision tree JSON file.
    tree: PathBuf,
    /// Path to config directory (agents.toml, invariants.toml).
    config_dir: PathBuf,
    /// Path to SQLite database.
    db: PathBuf,
    /// Log level filter.
    log_level: String,
}

impl CliArgs {
    /// Parse CLI arguments from `std::env::args`.
    ///
    /// Supports:
    ///   --tree <PATH>       [default: models/agent_policy_tree.json]
    ///   --config <DIR>      [default: config/]
    ///   --db <PATH>         [default: arbiter.db]
    ///   --log-level <LEVEL> [default: info]
    ///   --version
    ///   --help
    fn parse() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let mut tree = PathBuf::from("models/agent_policy_tree.json");
        let mut config_dir = PathBuf::from("config/");
        let mut db = PathBuf::from("arbiter.db");
        let mut log_level = "info".to_string();

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--tree" => {
                    i += 1;
                    if i < args.len() {
                        tree = PathBuf::from(&args[i]);
                    } else {
                        eprintln!("error: --tree requires a value");
                        process::exit(1);
                    }
                }
                "--config" => {
                    i += 1;
                    if i < args.len() {
                        config_dir = PathBuf::from(&args[i]);
                    } else {
                        eprintln!("error: --config requires a value");
                        process::exit(1);
                    }
                }
                "--db" => {
                    i += 1;
                    if i < args.len() {
                        db = PathBuf::from(&args[i]);
                    } else {
                        eprintln!("error: --db requires a value");
                        process::exit(1);
                    }
                }
                "--log-level" => {
                    i += 1;
                    if i < args.len() {
                        log_level = args[i].clone();
                    } else {
                        eprintln!("error: --log-level requires a value");
                        process::exit(1);
                    }
                }
                "--version" => {
                    eprintln!("arbiter {}", env!("CARGO_PKG_VERSION"));
                    process::exit(0);
                }
                "--help" | "-h" => {
                    print_help();
                    process::exit(0);
                }
                other => {
                    eprintln!("error: unknown argument: {other}");
                    print_help();
                    process::exit(1);
                }
            }
            i += 1;
        }

        Self {
            tree,
            config_dir,
            db,
            log_level,
        }
    }
}

/// Print usage help to stderr.
fn print_help() {
    eprintln!(
        "arbiter — Coding Agent Policy Engine (MCP Server)

USAGE:
    arbiter [OPTIONS]

OPTIONS:
    --tree <PATH>       Path to decision tree JSON
                        [default: models/agent_policy_tree.json]
    --config <DIR>      Path to config directory
                        [default: config/]
    --db <PATH>         Path to SQLite database
                        [default: arbiter.db]
    --log-level <LEVEL> Log level: trace|debug|info|warn|error
                        [default: info]
    --version           Print version
    --help              Print help"
    );
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize tracing subscriber with stderr output.
fn init_tracing(level: &str) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[allow(clippy::arc_with_non_send_sync)]
fn main() {
    let args = CliArgs::parse();

    init_tracing(&args.log_level);

    info!(
        tree = %args.tree.display(),
        config = %args.config_dir.display(),
        db = %args.db.display(),
        "starting arbiter"
    );

    // Load configuration
    let config = match config::load_config(&args.config_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e:#}");
            process::exit(1);
        }
    };

    info!(agents = config.agents.len(), "configuration loaded");

    // Load decision tree (optional — runs in degraded round-robin mode if unavailable)
    let tree = match std::fs::read_to_string(&args.tree) {
        Ok(json) => match DecisionTree::from_json(&json) {
            Ok(t) => {
                info!(
                    nodes = t.node_count(),
                    depth = t.depth(),
                    classes = t.n_classes(),
                    "decision tree loaded"
                );
                Some(t)
            }
            Err(e) => {
                eprintln!(
                    "WARNING: failed to parse decision tree: {e:#}. \
                     Running in degraded round-robin mode."
                );
                None
            }
        },
        Err(e) => {
            eprintln!(
                "WARNING: failed to read decision tree {}: {e}. \
                 Running in degraded round-robin mode.",
                args.tree.display()
            );
            None
        }
    };

    // Open SQLite database
    let database = match db::Database::open(&args.db) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open database {}: {e:#}", args.db.display());
            process::exit(1);
        }
    };
    if let Err(e) = database.migrate() {
        eprintln!("failed to migrate database: {e:#}");
        process::exit(1);
    }
    info!(path = %args.db.display(), "database ready");

    // Wrap in Arc for shared ownership (hot-reload ownership model).
    let database = Arc::new(database);

    // Retention: purge records older than 90 days on startup.
    match database.purge_older_than(90) {
        Ok(n) if n > 0 => info!(deleted = n, "startup retention purge"),
        Ok(_) => {}
        Err(e) => eprintln!("WARNING: retention purge failed: {e:#}"),
    }

    // Crash recovery: reset any orphaned running_tasks counters.
    match database.reset_all_running_tasks() {
        Ok(n) if n > 0 => info!(agents_reset = n, "startup: reset orphaned running_tasks"),
        Ok(_) => {}
        Err(e) => eprintln!("WARNING: running_tasks reset failed: {e:#}"),
    }

    // Create agent registry
    let registry = match agents::AgentRegistry::new(Arc::clone(&database), &config.agents) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to create agent registry: {e:#}");
            process::exit(1);
        }
    };

    // Create metrics collector
    let metrics = Arc::new(arbiter_mcp::metrics::Metrics::new());

    // Wrap hot-reloadable state in Arc<RwLock>
    let config = Arc::new(RwLock::new(config));
    let tree = Arc::new(RwLock::new(tree));
    let registry = Arc::new(RwLock::new(registry));

    // Create shutdown flag and register signal handlers
    let shutdown = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        if let Err(e) = signal_hook::flag::register(sig, Arc::clone(&shutdown)) {
            eprintln!("WARNING: failed to register signal handler: {e}");
        }
    }

    // Start file watcher for hot-reloading config and decision tree.
    let _watcher = match arbiter_mcp::watcher::start_watcher(
        arbiter_mcp::watcher::WatchPaths {
            config_dir: args.config_dir.clone(),
            tree_path: args.tree.clone(),
        },
        arbiter_mcp::watcher::ReloadableState {
            config: Arc::clone(&config),
            tree: Arc::clone(&tree),
            registry: Arc::clone(&registry),
            db: Arc::clone(&database),
        },
    ) {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!(
                "WARNING: file watcher failed to start: {e:#}. \
                 Hot reload disabled."
            );
            None
        }
    };

    // Create and run MCP server
    let mut server = server::McpServer::new(
        Arc::clone(&config),
        Arc::clone(&database),
        Arc::clone(&tree),
        Arc::clone(&registry),
        Arc::clone(&metrics),
        shutdown,
    );
    if let Err(e) = server.run() {
        eprintln!("server error: {e:#}");
        process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[test]
    fn arbiter_mcp_compiles() {
        assert!(true);
    }
}
