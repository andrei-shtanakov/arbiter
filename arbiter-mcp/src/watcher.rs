//! File watcher for hot-reloading config and decision tree.
//!
//! Watches the config directory for `.toml` changes and the decision tree
//! JSON file for updates. On detected changes, reloads the corresponding
//! state behind `Arc<RwLock<>>` without restarting the server.
//!
//! Uses a channel-based design: the `notify` callback (which must be
//! `Send + Sync`) forwards lightweight events to a processing thread
//! that owns the `ReloadableState` (including the non-`Sync` `Database`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::thread;

use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{info, warn};

use arbiter_core::policy::decision_tree::DecisionTree;

use crate::agents::AgentRegistry;
use crate::config::{self, ArbiterConfig};
use crate::db::Database;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Paths to watch for changes.
pub struct WatchPaths {
    /// Directory containing `agents.toml` and `invariants.toml`.
    pub config_dir: PathBuf,
    /// Path to the decision tree JSON file.
    pub tree_path: PathBuf,
}

/// Shared mutable state that the watcher can reload.
pub struct ReloadableState {
    /// Current arbiter configuration.
    pub config: Arc<RwLock<ArbiterConfig>>,
    /// Current decision tree (None = degraded round-robin mode).
    pub tree: Arc<RwLock<Option<DecisionTree>>>,
    /// Agent registry built from config + database.
    pub registry: Arc<RwLock<AgentRegistry>>,
    /// Database handle (needed to rebuild the registry).
    pub db: Arc<Database>,
}

// ---------------------------------------------------------------------------
// Path matching helpers
// ---------------------------------------------------------------------------

/// Check whether `path` is a `.toml` file inside `config_dir`.
pub fn path_matches_config(path: &Path, config_dir: &Path) -> bool {
    let ext_ok = path.extension().is_some_and(|ext| ext == "toml");
    let dir_ok = path.parent() == Some(config_dir);
    ext_ok && dir_ok
}

/// Check whether `path` is exactly the decision tree file.
pub fn path_matches_tree(path: &Path, tree_path: &Path) -> bool {
    path == tree_path
}

// ---------------------------------------------------------------------------
// Reload logic
// ---------------------------------------------------------------------------

/// Reload config from disk and update shared state.
fn reload_config(config_dir: &Path, state: &ReloadableState) {
    info!(dir = %config_dir.display(), "reloading configuration");

    let new_config = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => {
            warn!(
                error = %format!("{e:#}"),
                "failed to reload config; keeping previous"
            );
            return;
        }
    };

    // Rebuild registry with new agent definitions.
    let new_registry = match AgentRegistry::new(Arc::clone(&state.db), &new_config.agents) {
        Ok(r) => r,
        Err(e) => {
            warn!(
                error = %format!("{e:#}"),
                "failed to rebuild agent registry; keeping previous"
            );
            return;
        }
    };

    // Swap config first, then registry.
    match state.config.write() {
        Ok(mut guard) => *guard = new_config,
        Err(e) => {
            warn!(error = %e, "config RwLock poisoned");
            return;
        }
    }
    match state.registry.write() {
        Ok(mut guard) => *guard = new_registry,
        Err(e) => {
            warn!(error = %e, "registry RwLock poisoned");
            return;
        }
    }

    info!("configuration reloaded successfully");
}

/// Reload decision tree from disk and update shared state.
fn reload_tree(tree_path: &Path, state: &ReloadableState) {
    info!(path = %tree_path.display(), "reloading decision tree");

    let json = match std::fs::read_to_string(tree_path) {
        Ok(j) => j,
        Err(e) => {
            warn!(
                error = %e,
                "failed to read decision tree file; keeping previous"
            );
            return;
        }
    };

    let new_tree = match DecisionTree::from_json(&json).map_err(|e| anyhow::anyhow!("{e}")) {
        Ok(t) => t,
        Err(e) => {
            warn!(
                error = %format!("{e:#}"),
                "failed to parse decision tree; keeping previous"
            );
            return;
        }
    };

    info!(
        nodes = new_tree.node_count(),
        depth = new_tree.depth(),
        classes = new_tree.n_classes(),
        "decision tree parsed"
    );

    match state.tree.write() {
        Ok(mut guard) => *guard = Some(new_tree),
        Err(e) => {
            warn!(error = %e, "tree RwLock poisoned");
        }
    }

    info!("decision tree reloaded successfully");
}

// ---------------------------------------------------------------------------
// Internal reload event
// ---------------------------------------------------------------------------

/// Which kind of reload the processing thread should perform.
enum ReloadKind {
    /// Reload TOML config and rebuild agent registry.
    Config,
    /// Reload decision tree JSON.
    Tree,
}

// ---------------------------------------------------------------------------
// Watcher entry point
// ---------------------------------------------------------------------------

/// Start a file-system watcher that hot-reloads config and decision tree.
///
/// Returns the watcher handle. The caller must keep it alive (dropping
/// it stops the watcher). Errors during reload are logged but never
/// crash the server.
///
/// Internally spawns a processing thread that owns the `ReloadableState`
/// because `Database` is `!Sync` and `Arc<Database>` cannot be moved
/// into the `notify` callback directly.
#[allow(clippy::arc_with_non_send_sync)]
pub fn start_watcher(paths: WatchPaths, state: ReloadableState) -> Result<RecommendedWatcher> {
    // Canonicalize paths for reliable comparison.
    let config_dir = paths
        .config_dir
        .canonicalize()
        .unwrap_or_else(|_| paths.config_dir.clone());
    let tree_path = paths
        .tree_path
        .canonicalize()
        .unwrap_or_else(|_| paths.tree_path.clone());

    // Channel: notify callback -> processing thread.
    let (tx, rx) = std::sync::mpsc::channel::<ReloadKind>();

    // Clones for the notify callback (lightweight, no Database).
    let config_dir_cb = config_dir.clone();
    let tree_path_cb = tree_path.clone();

    let mut watcher =
        notify::recommended_watcher(move |res: std::result::Result<Event, notify::Error>| {
            let event = match res {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "file watcher error");
                    return;
                }
            };

            // Only react to create/modify events.
            match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) => {}
                _ => return,
            }

            for path in &event.paths {
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());

                if path_matches_config(&canonical, &config_dir_cb) {
                    let _ = tx.send(ReloadKind::Config);
                } else if path_matches_tree(&canonical, &tree_path_cb) {
                    let _ = tx.send(ReloadKind::Tree);
                }
            }
        })
        .context("failed to create file watcher")?;

    // Processing thread: owns ReloadableState (including Arc<Database>).
    let config_dir_proc = config_dir.clone();
    let tree_path_proc = tree_path.clone();
    thread::Builder::new()
        .name("arbiter-reload".into())
        .spawn(move || {
            while let Ok(kind) = rx.recv() {
                match kind {
                    ReloadKind::Config => {
                        reload_config(&config_dir_proc, &state);
                    }
                    ReloadKind::Tree => {
                        reload_tree(&tree_path_proc, &state);
                    }
                }
            }
            // Channel closed (watcher dropped) — thread exits.
        })
        .context("failed to spawn reload thread")?;

    // Watch config directory.
    watcher
        .watch(&config_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("failed to watch config dir: {}", config_dir.display()))?;

    // Watch tree file's parent directory (notify cannot watch
    // individual files reliably on all platforms).
    if let Some(tree_parent) = tree_path.parent() {
        watcher
            .watch(tree_parent, RecursiveMode::NonRecursive)
            .with_context(|| format!("failed to watch tree dir: {}", tree_parent.display()))?;
    }

    info!(
        config_dir = %config_dir.display(),
        tree_path = %tree_path.display(),
        "file watcher started"
    );

    Ok(watcher)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn path_matches_config_toml_in_dir() {
        let config_dir = PathBuf::from("/tmp/config");
        let path = PathBuf::from("/tmp/config/agents.toml");
        assert!(path_matches_config(&path, &config_dir));
    }

    #[test]
    fn path_matches_config_rejects_non_toml() {
        let config_dir = PathBuf::from("/tmp/config");
        let path = PathBuf::from("/tmp/config/README.md");
        assert!(!path_matches_config(&path, &config_dir));
    }

    #[test]
    fn path_matches_config_rejects_wrong_dir() {
        let config_dir = PathBuf::from("/tmp/config");
        let path = PathBuf::from("/tmp/other/agents.toml");
        assert!(!path_matches_config(&path, &config_dir));
    }

    #[test]
    fn path_matches_tree_exact() {
        let tree = PathBuf::from("/tmp/models/tree.json");
        let path = PathBuf::from("/tmp/models/tree.json");
        assert!(path_matches_tree(&path, &tree));
    }

    #[test]
    fn path_matches_tree_rejects_other() {
        let tree = PathBuf::from("/tmp/models/tree.json");
        let path = PathBuf::from("/tmp/models/other.json");
        assert!(!path_matches_tree(&path, &tree));
    }
}
