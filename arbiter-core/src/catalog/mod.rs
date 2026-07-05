//! User-config agents-catalog loader (ADR-ECO-003b).
//!
//! Pure logic: parsing, validation and path resolution for the
//! user-configured agents catalog (`$ATP_CATALOG` / XDG). No file I/O —
//! callers read the file and inject env access.
//!
//! Design: `docs/2026-07-05-catalog-loader-design.md`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

/// Env var holding an explicit catalog path (resolution layer 1).
pub const CATALOG_ENV_VAR: &str = "ATP_CATALOG";
/// Catalog path relative to the XDG config dir (resolution layers 2-3).
pub const XDG_SUBPATH: &str = "atp/agents-catalog.toml";

/// Errors produced by catalog parsing and path resolution.
#[derive(Debug, Error)]
pub enum CatalogError {
    /// No configuration layer present — fail-loud per ADR-003b D2.
    #[error(
        "model catalog not configured: set $ATP_CATALOG or create \
         ~/.config/atp/agents-catalog.toml"
    )]
    NotConfigured,
    /// $ATP_CATALOG points at a file that does not exist (no fallback).
    #[error("catalog file not found: {path} (from $ATP_CATALOG)")]
    EnvFileNotFound {
        /// The path taken from `$ATP_CATALOG`.
        path: PathBuf,
    },
    /// TOML syntax or shape error.
    #[error("invalid catalog TOML: {0}")]
    Parse(#[from] toml::de::Error),
    /// Structurally valid TOML but no catalog content at all.
    #[error("catalog is empty: no models, harnesses or agents declared")]
    Empty,
}

/// Model lifecycle status (Plane 1). Unknown values degrade to `Other`
/// (forward-compat); consumers MUST allowlist `Active`, not denylist
/// `Retired` — see design §3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelStatus {
    /// Model is live and may be enrolled/routed.
    Active,
    /// Model is being phased out (warning V6 on live references).
    Deprecated,
    /// Model must not be referenced by enrollment (error V3).
    Retired,
    /// Unrecognized status value — degrade-with-warning (V7).
    Other(String),
}

impl<'de> Deserialize<'de> for ModelStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "active" => Self::Active,
            "deprecated" => Self::Deprecated,
            "retired" => Self::Retired,
            _ => Self::Other(s),
        })
    }
}

impl fmt::Display for ModelStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Deprecated => write!(f, "deprecated"),
            Self::Retired => write!(f, "retired"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

/// Harness launch mechanics kind (Plane 2). Unknown values degrade to
/// `Other` (informational only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessKind {
    /// CLI coding agent.
    Cli,
    /// Raw-API baseline (never routed).
    ApiBaseline,
    /// Local model runner (never routed).
    Local,
    /// Unrecognized kind value — degrade-with-warning (V7).
    Other(String),
}

impl<'de> Deserialize<'de> for HarnessKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "cli" => Self::Cli,
            "api-baseline" => Self::ApiBaseline,
            "local" => Self::Local,
            _ => Self::Other(s),
        })
    }
}

/// Plane 1: a model entry (vendor lifecycle facts).
#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    /// Vendor name, e.g. `anthropic`.
    pub vendor: String,
    /// Lifecycle status.
    pub status: ModelStatus,
    /// Alternative model ids.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Plane 2: a harness entry (launch mechanics).
#[derive(Debug, Clone, Deserialize)]
pub struct HarnessEntry {
    /// Launch mechanics kind.
    pub kind: HarnessKind,
    /// ATP-side shim path (informational for arbiter).
    pub shim: String,
    /// Env var the ATP shim uses to pin the model.
    #[serde(default)]
    pub model_env: Option<String>,
    /// Model CLI flag on the Maestro spawner side, where present.
    #[serde(default)]
    pub model_flag: Option<String>,
    /// Whether the harness can be routed at all (requires a Maestro spawner).
    #[serde(default)]
    pub routable: bool,
}

/// Plane 3: an enrollment entry for a (harness, model) pair.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentEntry {
    /// Harness key (must be declared in Plane 2).
    pub harness: String,
    /// Model key (must be declared in Plane 1).
    pub model: String,
    /// Whether the pair is enrolled in the ATP sweep.
    #[serde(default)]
    pub tested: bool,
    /// Whether the pair is promoted into routing (manual gate flip).
    #[serde(default)]
    pub routable: bool,
}

impl AgentEntry {
    /// Canonical join key: `"{harness}@{model}"` (byte-for-byte, matches
    /// `benchmark_runs.agent_id`).
    pub fn agent_id(&self) -> String {
        format!("{}@{}", self.harness, self.model)
    }
}

/// The three-plane agents catalog (ADR-ECO-003).
#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    /// Plane 1: models by id.
    #[serde(default)]
    pub models: BTreeMap<String, ModelEntry>,
    /// Plane 2: harnesses by key.
    #[serde(default)]
    pub harnesses: BTreeMap<String, HarnessEntry>,
    /// Plane 3: enrollment entries.
    #[serde(default)]
    pub agents: Vec<AgentEntry>,
}

/// Parse catalog TOML text. Unknown fields are ignored (forward-compat);
/// unknown `status`/`kind` values degrade to `Other` (validated as V7).
/// A structurally empty catalog is an error (`CatalogError::Empty`).
pub fn parse_catalog(toml_text: &str) -> Result<Catalog, CatalogError> {
    let catalog: Catalog = toml::from_str(toml_text)?;
    if catalog.models.is_empty() && catalog.harnesses.is_empty() && catalog.agents.is_empty() {
        return Err(CatalogError::Empty);
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
        [models."m-1"]
        vendor = "acme"
        status = "active"

        [harnesses.h1]
        kind = "cli"
        shim = "shims/h1.py"
        model_env = "H1_MODEL"
        routable = true

        [[agents]]
        harness = "h1"
        model = "m-1"
        tested = true
        routable = true
    "#;

    #[test]
    fn parses_minimal_catalog() {
        let cat = parse_catalog(MINIMAL).expect("minimal catalog must parse");
        assert_eq!(cat.models.len(), 1);
        assert_eq!(cat.models["m-1"].vendor, "acme");
        assert_eq!(cat.models["m-1"].status, ModelStatus::Active);
        assert_eq!(cat.harnesses["h1"].kind, HarnessKind::Cli);
        assert!(cat.harnesses["h1"].routable);
        assert_eq!(cat.agents.len(), 1);
        assert_eq!(cat.agents[0].agent_id(), "h1@m-1");
    }

    #[test]
    fn unknown_enum_values_degrade_to_other() {
        let text = MINIMAL
            .replace("\"active\"", "\"preview\"")
            .replace("\"cli\"", "\"container\"");
        let cat = parse_catalog(&text).expect("unknown enum must not fail parse");
        assert_eq!(
            cat.models["m-1"].status,
            ModelStatus::Other("preview".to_string())
        );
        assert_eq!(
            cat.harnesses["h1"].kind,
            HarnessKind::Other("container".to_string())
        );
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let text = format!("{MINIMAL}\n[models.\"m-1\".extra_section]\nfoo = 1\n");
        let with_field = text.replace(
            "vendor = \"acme\"",
            "vendor = \"acme\"\nfuture_field = \"x\"",
        );
        parse_catalog(&with_field).expect("unknown fields must be ignored");
    }

    #[test]
    fn empty_file_is_an_error() {
        assert!(matches!(parse_catalog(""), Err(CatalogError::Empty)));
    }

    #[test]
    fn broken_toml_is_a_parse_error() {
        assert!(matches!(
            parse_catalog("[models\nboom"),
            Err(CatalogError::Parse(_))
        ));
    }

    #[test]
    fn optional_fields_default() {
        // model_flag/model_env отсутствуют, tested/routable отсутствуют.
        let text = r#"
            [models."m"]
            vendor = "v"
            status = "active"

            [harnesses.h]
            kind = "local"
            shim = "s.py"

            [[agents]]
            harness = "h"
            model = "m"
        "#;
        let cat = parse_catalog(text).unwrap();
        assert!(!cat.harnesses["h"].routable);
        assert!(cat.harnesses["h"].model_env.is_none());
        assert!(cat.harnesses["h"].model_flag.is_none());
        assert!(!cat.agents[0].tested);
        assert!(!cat.agents[0].routable);
    }
}
