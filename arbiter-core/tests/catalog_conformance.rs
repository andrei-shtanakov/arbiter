//! Cross-loader conformance suite for the user-config agents catalog.
//!
//! Runs `arbiter_core::catalog` against the vendored pinned copy of the
//! SSOT fixture set owned by devtools
//! (`tests/fixtures/catalog-conformance/v1/`, see its `PIN`). ADR-ECO-003b
//! risk #1: the three loaders (Maestro / ATP / arbiter) may drift in
//! behaviour; this set makes the drift observable. arbiter is the reference
//! implementation of rules V1..V7, so every negative case is asserted down
//! to the rule code, not merely "rejected somehow".
//!
//! Accepted from inbox issue #74 (slug `catalog-conformance-fixtures`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use arbiter_core::catalog::{
    self, Catalog, CatalogError, CatalogSource, Issue, ResolvedPath, Severity,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Root of the vendored fixture set.
fn set_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalog-conformance/v1")
}

// ---------------------------------------------------------------------------
// expectations.toml
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Expectations {
    version: u32,
    #[serde(default)]
    case: Vec<Case>,
    #[serde(default)]
    pathres: Vec<PathRes>,
}

#[derive(Debug, Deserialize)]
struct Case {
    file: String,
    expect: String,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PathRes {
    id: String,
    env: String,
    #[serde(default)]
    target: Option<String>,
    expect: String,
}

fn expectations() -> Expectations {
    let path = set_dir().join("expectations.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("expectations.toml must parse: {e}"))
}

// ---------------------------------------------------------------------------
// Pin surface: the vendored copy must be byte-identical to the owner's set
// ---------------------------------------------------------------------------

/// Every file in the pin surface, relative to the set root, sorted.
/// `manifest.json` (the surface description) and `PIN` (ours, not the
/// owner's) are excluded — see `manifest.json.surface_note`.
fn surface_files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("path under root")
                    .to_str()
                    .expect("utf-8 path")
                    .replace('\\', "/");
                if rel != "manifest.json" && rel != "PIN" {
                    out.push(rel);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn sha256_file(path: &Path) -> String {
    let bytes =
        std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn vendored_copy_matches_the_pinned_manifest() {
    let root = set_dir();
    let manifest_path = root.join("manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("cannot read manifest: {e}")),
    )
    .expect("manifest.json must parse");

    assert_eq!(manifest["contract"], "catalog-conformance-fixtures");
    assert_eq!(manifest["contract_version"], "v1");

    let declared: Vec<(String, String)> = manifest["files"]
        .as_array()
        .expect("manifest.files must be an array")
        .iter()
        .map(|e| {
            (
                e["path"].as_str().expect("file path").to_string(),
                e["sha256"].as_str().expect("file sha256").to_string(),
            )
        })
        .collect();

    // Same file set, in the same canonical (sorted) order: catches an added,
    // removed or renamed file as well as a reordered manifest.
    let on_disk = surface_files(&root);
    let declared_paths: Vec<String> = declared.iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(
        on_disk, declared_paths,
        "vendored file set differs from the manifest surface"
    );

    // Per-file digests.
    for (rel, want) in &declared {
        let got = sha256_file(&root.join(rel));
        assert_eq!(&got, want, "sha256 mismatch for {rel}");
    }

    // tree_sha256 = sha256 over the sorted "<path> <sha256>\n" lines.
    let tree: String = declared
        .iter()
        .map(|(p, h)| format!("{p} {h}\n"))
        .collect::<Vec<_>>()
        .concat();
    let tree_digest = format!("{:x}", Sha256::digest(tree.as_bytes()));
    assert_eq!(
        tree_digest,
        manifest["tree_sha256"].as_str().expect("tree_sha256"),
        "tree_sha256 mismatch — the vendored copy is not the pinned set"
    );
}

// ---------------------------------------------------------------------------
// [[case]] — one test body per expectation class
// ---------------------------------------------------------------------------

fn codes(issues: &[Issue], severity: Severity) -> Vec<&str> {
    let mut out: Vec<&str> = issues
        .iter()
        .filter(|i| i.severity == severity)
        .map(|i| i.code)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn read_fixture(rel: &str) -> String {
    let path = set_dir().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Assert one `[[case]]`; returns a failure description instead of panicking
/// so a single run reports every divergence at once.
fn check_case(case: &Case) -> Result<(), String> {
    let text = read_fixture(&case.file);
    let parsed = catalog::parse_catalog(&text);
    let file = &case.file;

    match case.expect.as_str() {
        // Loads with neither errors nor warnings.
        "valid" => {
            let cat = parsed.map_err(|e| format!("{file}: must parse, got {e}"))?;
            let issues = catalog::validate(&cat);
            if !issues.is_empty() {
                return Err(format!("{file}: expected no issues, got {issues:?}"));
            }
        }
        // Must not parse as TOML at all.
        "parse-error" => match parsed {
            Err(CatalogError::Parse(_)) => {}
            Err(other) => return Err(format!("{file}: expected a TOML parse error, got {other}")),
            Ok(_) => return Err(format!("{file}: expected a parse error, but it parsed")),
        },
        // Rejected, and by the named rule: arbiter is the reference
        // implementation, so a bare "rejected somehow" would let a wrong
        // reason (e.g. a schema slip) masquerade as conformance.
        "error" => {
            let want = case.code.as_deref().ok_or("error case without `code`")?;
            let cat = parsed.map_err(|e| format!("{file}: must parse to reach validation: {e}"))?;
            let issues = catalog::validate(&cat);
            let got = codes(&issues, Severity::Error);
            if got != vec![want] {
                return Err(format!("{file}: expected errors [{want}], got {got:?}"));
            }
        }
        // Flagged, not rejected. The contract also accepts outright
        // rejection here; asserting zero errors pins arbiter's own choice
        // (degrade-with-warning, design §3) as a regression guard.
        "flag" => {
            let want = case.code.as_deref().ok_or("flag case without `code`")?;
            let cat = parsed.map_err(|e| format!("{file}: must parse to reach validation: {e}"))?;
            let issues = catalog::validate(&cat);
            let errors = codes(&issues, Severity::Error);
            if !errors.is_empty() {
                return Err(format!("{file}: expected no errors, got {errors:?}"));
            }
            let warnings = codes(&issues, Severity::Warning);
            if !warnings.contains(&want) {
                return Err(format!("{file}: expected warning {want}, got {warnings:?}"));
            }
        }
        other => return Err(format!("{file}: unknown expectation class {other:?}")),
    }
    Ok(())
}

#[test]
fn every_case_conforms() {
    let exp = expectations();
    assert_eq!(exp.version, 1, "vendored set is not v1");
    assert!(!exp.case.is_empty(), "no [[case]] rows — truncated copy?");

    let failures: Vec<String> = exp
        .case
        .iter()
        .filter_map(|c| check_case(c).err())
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} conformance case(s) failed:\n  {}",
        failures.len(),
        exp.case.len(),
        failures.join("\n  ")
    );
}

#[test]
fn the_set_exercises_every_rule_v1_through_v7() {
    // Guards against a vendored copy that is intact per the manifest but
    // silently stops covering a rule after a pin bump.
    let exp = expectations();
    let covered: BTreeSet<&str> = exp.case.iter().filter_map(|c| c.code.as_deref()).collect();
    let want: BTreeSet<&str> = ["V1", "V2", "V3", "V4", "V5", "V6", "V7"]
        .into_iter()
        .collect();
    assert_eq!(covered, want, "rule coverage changed");
}

#[test]
fn v7_fires_on_both_planes_not_just_one() {
    // The set's `flag` class only requires that V7 be raised at all; the
    // v7 fixture carries an unknown model status AND an unknown harness
    // kind, so arbiter asserts both degrade paths still fire.
    let text = read_fixture("fixtures/warn/v7-unknown-enum.toml");
    let cat = catalog::parse_catalog(&text).expect("v7 fixture must parse");
    let issues = catalog::validate(&cat);
    let v7: Vec<&Issue> = issues.iter().filter(|i| i.code == "V7").collect();
    assert_eq!(v7.len(), 2, "expected a model and a harness V7: {v7:?}");
    assert!(
        v7.iter().any(|i| i.message.contains("status")),
        "no V7 for the unknown model status: {v7:?}"
    );
    assert!(
        v7.iter().any(|i| i.message.contains("kind")),
        "no V7 for the unknown harness kind: {v7:?}"
    );
}

// ---------------------------------------------------------------------------
// [[pathres]] — the $ATP_CATALOG resolution layer (ADR-ECO-003b D2)
// ---------------------------------------------------------------------------

/// Outcome of the caller-side load composition, mirroring
/// `arbiter-cli::load_catalog` and `arbiter-mcp::catalog_guard`:
/// resolve → check existence → read → parse.
enum LoadOutcome {
    Loaded(Catalog),
    Failed(CatalogError),
}

/// `env` is injected rather than set on the process: `resolve_path` is pure
/// by design, and real env mutation is not safe across parallel test threads.
fn load_with_env(env: &[(&str, &str)], home: Option<&Path>) -> LoadOutcome {
    let lookup = |key: &str| {
        env.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_string())
    };
    let resolved: ResolvedPath = match catalog::resolve_path(lookup, home) {
        Ok(r) => r,
        Err(e) => return LoadOutcome::Failed(e),
    };
    if !resolved.path.exists() {
        return LoadOutcome::Failed(catalog::missing_file_error(&resolved));
    }
    let text = match std::fs::read_to_string(&resolved.path) {
        Ok(t) => t,
        Err(e) => panic!("resolved path unreadable: {e}"),
    };
    match catalog::parse_catalog(&text) {
        Ok(cat) => LoadOutcome::Loaded(cat),
        Err(e) => LoadOutcome::Failed(e),
    }
}

fn check_pathres(scenario: &PathRes, missing_path: &Path) -> Result<(), String> {
    let id = &scenario.id;
    let outcome = match scenario.env.as_str() {
        "set" => {
            let target = scenario
                .target
                .as_deref()
                .ok_or_else(|| format!("{id}: env=set without `target`"))?;
            let path = set_dir().join(target);
            let path = path.to_str().ok_or("non-utf8 fixture path")?;
            load_with_env(&[(catalog::CATALOG_ENV_VAR, path)], None)
        }
        // No layer configured at all. arbiter's XDG layers sit below this
        // one and are outside the shared v1 set, so `home` is None here.
        "unset" => load_with_env(&[], None),
        "set-missing" => load_with_env(
            &[(
                catalog::CATALOG_ENV_VAR,
                missing_path.to_str().ok_or("non-utf8 temp path")?,
            )],
            None,
        ),
        other => return Err(format!("{id}: unknown env mode {other:?}")),
    };

    match (scenario.expect.as_str(), outcome) {
        ("loaded", LoadOutcome::Loaded(cat)) => {
            if cat.agents.is_empty() {
                return Err(format!("{id}: loaded an empty catalog"));
            }
            Ok(())
        }
        ("not-configured", LoadOutcome::Failed(CatalogError::NotConfigured)) => Ok(()),
        // The non-conformance this case exists to catch is a loader that
        // treats a missing $ATP_CATALOG target as "simply not configured".
        ("set-missing", _) => Err(format!("{id}: `set-missing` is an env mode, not a class")),
        ("missing-file-error", LoadOutcome::Failed(CatalogError::EnvFileNotFound { path })) => {
            if !path.to_string_lossy().contains("nonexistent") {
                return Err(format!(
                    "{id}: error names the wrong path: {}",
                    path.display()
                ));
            }
            Ok(())
        }
        (want, LoadOutcome::Loaded(_)) => {
            Err(format!("{id}: expected {want}, but the catalog loaded"))
        }
        (want, LoadOutcome::Failed(e)) => Err(format!("{id}: expected {want}, got error: {e}")),
    }
}

#[test]
fn every_path_resolution_scenario_conforms() {
    let exp = expectations();
    assert!(
        !exp.pathres.is_empty(),
        "no [[pathres]] rows — truncated copy?"
    );

    let tmp = tempfile::tempdir().expect("temp dir");
    let missing = tmp.path().join("nonexistent-catalog.toml");
    assert!(!missing.exists());

    let failures: Vec<String> = exp
        .pathres
        .iter()
        .filter_map(|s| check_pathres(s, &missing).err())
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} path-resolution scenario(s) failed:\n  {}",
        failures.len(),
        exp.pathres.len(),
        failures.join("\n  ")
    );
}

#[test]
fn explicit_env_miss_is_not_reported_as_unconfigured() {
    // The single divergence the pathres block is aimed at (Maestro returns a
    // silent None here). Spelled out separately so the failure message names
    // the behaviour rather than a scenario id.
    let tmp = tempfile::tempdir().expect("temp dir");
    let missing = tmp.path().join("nonexistent-catalog.toml");
    let outcome = load_with_env(
        &[(catalog::CATALOG_ENV_VAR, missing.to_str().expect("utf-8"))],
        Some(Path::new("/home/somebody")),
    );
    match outcome {
        LoadOutcome::Failed(CatalogError::EnvFileNotFound { path }) => assert_eq!(path, missing),
        LoadOutcome::Failed(other) => panic!("expected EnvFileNotFound, got {other}"),
        LoadOutcome::Loaded(_) => panic!("expected an error, catalog loaded"),
    }
}

#[test]
fn explicit_env_layer_wins_over_every_lower_layer() {
    // $ATP_CATALOG has no fallback beneath it (ADR-ECO-003b D2): the XDG
    // layers must not rescue a resolution that layer 1 already answered.
    let target = set_dir().join("fixtures/valid/three-planes.toml");
    let lookup = |key: &str| match key {
        catalog::CATALOG_ENV_VAR => Some(target.to_string_lossy().into_owned()),
        "XDG_CONFIG_HOME" => Some("/xdg".to_string()),
        _ => None,
    };
    let resolved = catalog::resolve_path(lookup, Some(Path::new("/home/somebody")))
        .expect("explicit env path resolves");
    assert_eq!(resolved.path, target);
    assert!(matches!(resolved.source, CatalogSource::AtpCatalogEnv));
}
