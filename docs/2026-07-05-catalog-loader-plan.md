# User-Config Catalog Loader — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rust-загрузчик user-config каталога агентов (ADR-ECO-003b): резолюция `$ATP_CATALOG` → XDG, парсинг трёх плоскостей, валидация V1–V7, CLI `arbiter-cli catalog path|check|list`, fail-loud без bundled-дефолта.

**Architecture:** Чистый модуль `arbiter_core::catalog` (типы + `parse_catalog` + `validate` + `resolve_path` с инжектированным env — без I/O). Весь I/O (чтение env/файла, печать, exit codes) — в `arbiter-cli`. Runtime `arbiter-mcp` не трогаем.

**Tech Stack:** Rust, serde + toml (оба уже в `[workspace.dependencies]`), thiserror. Никаких новых внешних зависимостей.

**Spec:** `docs/2026-07-05-catalog-loader-design.md` — источник требований. Одно уточнение относительно спеки: `resolve_path` возвращает `ResolvedPath { path, source }` (не голый `PathBuf`) — источник нужен, чтобы отличить «файл из `$ATP_CATALOG` не найден» от «не сконфигурировано» (§2 спеки требует разные сообщения).

## Global Constraints

- No `unwrap()` в production-путях (тесты — можно); ошибки через `thiserror`.
- Все pub-функции — с doc-комментами.
- `cargo fmt --all` и `cargo clippy --workspace -- -D warnings` должны быть зелёными после каждой задачи.
- CLI печатает результат в stdout, ошибки в stderr (CLI — не MCP-канал; правило «stdout только для протокола» касается arbiter-mcp).
- `config/agents-catalog.toml` runtime/CLI НЕ читает; его читает только happy-path-тест (тесты — не shipped-код).
- Никакого bundled-дефолта каталога; отсутствие конфига → ошибка с текстом, содержащим `$ATP_CATALOG` и `~/.config/atp/agents-catalog.toml`.
- Коммиты — на ветке `feat/catalog-loader`, футер `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task 1: Типы каталога + `parse_catalog` (arbiter-core)

**Files:**
- Modify: `arbiter-core/Cargo.toml` (добавить `toml = { workspace = true }` в `[dependencies]`)
- Modify: `arbiter-core/src/lib.rs` (добавить `pub mod catalog;` в алфавитном порядке — между `error` и `invariant`)
- Create: `arbiter-core/src/catalog/mod.rs`

**Interfaces:**
- Produces:
  - `pub struct Catalog { models: BTreeMap<String, ModelEntry>, harnesses: BTreeMap<String, HarnessEntry>, agents: Vec<AgentEntry> }`
  - `pub enum ModelStatus { Active, Deprecated, Retired, Other(String) }` (+ `Display`)
  - `pub enum HarnessKind { Cli, ApiBaseline, Local, Other(String) }`
  - `pub enum CatalogError { NotConfigured, EnvFileNotFound { path: PathBuf }, Parse(toml::de::Error), Empty }`
  - `pub fn parse_catalog(toml_text: &str) -> Result<Catalog, CatalogError>`
  - `impl AgentEntry { pub fn agent_id(&self) -> String }` — `"{harness}@{model}"`

- [ ] **Step 1: Написать падающие тесты парсинга**

Создать `arbiter-core/src/catalog/mod.rs` сразу с типами-заглушками НЕ надо — сначала тесты не скомпилируются, это и есть «красный». Создать файл только с `#[cfg(test)] mod tests` нельзя (нет типов), поэтому «красный» шаг здесь = написать файл целиком с тестами и МИНИМАЛЬНЫМИ пустыми типами, убедиться что тесты падают на ассертах/компиляции, затем дописать логику. Практически: записать в `arbiter-core/src/catalog/mod.rs` полный код из Step 3, но тело `parse_catalog` временно `unimplemented!()`, и прогнать тесты — они должны упасть.

Тесты (входят в файл Step 3, блок `#[cfg(test)]`):

```rust
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
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test -p arbiter-core catalog`
Expected: FAIL (паника `unimplemented!()` в `parse_catalog`).

- [ ] **Step 3: Полная реализация типов и `parse_catalog`**

`arbiter-core/Cargo.toml` — в `[dependencies]` добавить:

```toml
toml = { workspace = true }
```

`arbiter-core/src/lib.rs`:

```rust
pub mod catalog;
```

(строкой между `pub mod error;` и `pub mod invariant;`)

`arbiter-core/src/catalog/mod.rs` (без тестового блока из Step 1):

```rust
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
    if catalog.models.is_empty()
        && catalog.harnesses.is_empty()
        && catalog.agents.is_empty()
    {
        return Err(CatalogError::Empty);
    }
    Ok(catalog)
}
```

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test -p arbiter-core catalog`
Expected: PASS (6 тестов).

- [ ] **Step 5: fmt + clippy + коммит**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings
git add arbiter-core/Cargo.toml arbiter-core/src/lib.rs arbiter-core/src/catalog/mod.rs
git commit -m "feat(catalog): three-plane catalog types + parse_catalog (ADR-ECO-003b)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Валидация V1–V7 + фикстуры + happy-path на вендорном каталоге

**Files:**
- Modify: `arbiter-core/src/catalog/mod.rs` (добавить `Severity`, `Issue`, `validate`)
- Create: `arbiter-core/tests/fixtures/catalog/retired_ref.toml`
- Create: `arbiter-core/tests/fixtures/catalog/unknown_harness.toml`
- Create: `arbiter-core/tests/fixtures/catalog/unknown_model.toml`
- Create: `arbiter-core/tests/fixtures/catalog/dup_agent.toml`
- Create: `arbiter-core/tests/fixtures/catalog/routable_conflict.toml`
- Create: `arbiter-core/tests/fixtures/catalog/deprecated_ref.toml`
- Create: `arbiter-core/tests/fixtures/catalog/unknown_enum.toml`
- Create: `arbiter-core/tests/catalog_validation.rs`

**Interfaces:**
- Consumes: `Catalog`, `parse_catalog`, `ModelStatus`, `HarnessKind` из Task 1.
- Produces:
  - `pub enum Severity { Error, Warning }`
  - `pub struct Issue { pub severity: Severity, pub code: &'static str, pub message: String }`
  - `pub fn validate(catalog: &Catalog) -> Vec<Issue>` — собирает ВСЕ issues, не short-circuit.

- [ ] **Step 1: Создать фикстуры**

Общий скелет — каждая фикстура минимальна и бьёт ровно по одному правилу.

`arbiter-core/tests/fixtures/catalog/retired_ref.toml` (V3):

```toml
[models."old-model"]
vendor = "acme"
status = "retired"

[harnesses.h1]
kind = "cli"
shim = "shims/h1.py"
routable = true

[[agents]]
harness = "h1"
model = "old-model"
tested = true
```

`arbiter-core/tests/fixtures/catalog/unknown_harness.toml` (V1):

```toml
[models."m1"]
vendor = "acme"
status = "active"

[harnesses.h1]
kind = "cli"
shim = "shims/h1.py"

[[agents]]
harness = "ghost"
model = "m1"
```

`arbiter-core/tests/fixtures/catalog/unknown_model.toml` (V2):

```toml
[models."m1"]
vendor = "acme"
status = "active"

[harnesses.h1]
kind = "cli"
shim = "shims/h1.py"

[[agents]]
harness = "h1"
model = "ghost"
```

`arbiter-core/tests/fixtures/catalog/dup_agent.toml` (V4):

```toml
[models."m1"]
vendor = "acme"
status = "active"

[harnesses.h1]
kind = "cli"
shim = "shims/h1.py"

[[agents]]
harness = "h1"
model = "m1"

[[agents]]
harness = "h1"
model = "m1"
```

`arbiter-core/tests/fixtures/catalog/routable_conflict.toml` (V5):

```toml
[models."m1"]
vendor = "acme"
status = "active"

[harnesses.h1]
kind = "cli"
shim = "shims/h1.py"
routable = false

[[agents]]
harness = "h1"
model = "m1"
routable = true
```

`arbiter-core/tests/fixtures/catalog/deprecated_ref.toml` (V6, warning only):

```toml
[models."m1"]
vendor = "acme"
status = "deprecated"

[harnesses.h1]
kind = "cli"
shim = "shims/h1.py"
routable = true

[[agents]]
harness = "h1"
model = "m1"
```

`arbiter-core/tests/fixtures/catalog/unknown_enum.toml` (V7, warnings only):

```toml
[models."m1"]
vendor = "acme"
status = "preview"

[harnesses.h1]
kind = "container"
shim = "shims/h1.py"

[[agents]]
harness = "h1"
model = "m1"
```

- [ ] **Step 2: Написать падающие тесты валидации**

`arbiter-core/tests/catalog_validation.rs`:

```rust
//! Validation-rule tests on fixtures + happy-path on the vendored
//! dev-SSOT catalog copy. Fixtures are one-rule-per-file so they can be
//! shared with the ATP/Maestro loaders later (design §7-8).

use arbiter_core::catalog::{parse_catalog, validate, Severity};

fn load_fixture(name: &str) -> arbiter_core::catalog::Catalog {
    let path = format!(
        "{}/tests/fixtures/catalog/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    parse_catalog(&text).unwrap_or_else(|e| panic!("{name} must parse: {e}"))
}

fn codes(issues: &[arbiter_core::catalog::Issue], severity: Severity) -> Vec<&str> {
    issues
        .iter()
        .filter(|i| i.severity == severity)
        .map(|i| i.code)
        .collect()
}

#[test]
fn v1_unknown_harness_is_error() {
    let issues = validate(&load_fixture("unknown_harness.toml"));
    assert_eq!(codes(&issues, Severity::Error), vec!["V1"]);
}

#[test]
fn v2_unknown_model_is_error() {
    let issues = validate(&load_fixture("unknown_model.toml"));
    assert_eq!(codes(&issues, Severity::Error), vec!["V2"]);
}

#[test]
fn v3_retired_reference_is_error() {
    let issues = validate(&load_fixture("retired_ref.toml"));
    assert_eq!(codes(&issues, Severity::Error), vec!["V3"]);
}

#[test]
fn v4_duplicate_agent_id_is_error() {
    let issues = validate(&load_fixture("dup_agent.toml"));
    assert!(codes(&issues, Severity::Error).contains(&"V4"));
}

#[test]
fn v5_routable_agent_on_nonroutable_harness_is_error() {
    let issues = validate(&load_fixture("routable_conflict.toml"));
    assert_eq!(codes(&issues, Severity::Error), vec!["V5"]);
}

#[test]
fn v6_deprecated_reference_is_warning_not_error() {
    let issues = validate(&load_fixture("deprecated_ref.toml"));
    assert_eq!(codes(&issues, Severity::Error), Vec::<&str>::new());
    assert_eq!(codes(&issues, Severity::Warning), vec!["V6"]);
}

#[test]
fn v7_unknown_enum_values_are_warnings_not_errors() {
    let issues = validate(&load_fixture("unknown_enum.toml"));
    assert_eq!(codes(&issues, Severity::Error), Vec::<&str>::new());
    let warns = codes(&issues, Severity::Warning);
    assert_eq!(warns.iter().filter(|c| **c == "V7").count(), 2);
}

#[test]
fn vendored_dev_ssot_catalog_is_valid() {
    // Happy-path reads the in-repo vendored SSOT copy directly — NOT a
    // hand-maintained fixture copy (design §7: no third drifting artifact).
    // Assertions are invariants, not exact counts: the vendor file moves.
    let path = format!(
        "{}/../config/agents-catalog.toml",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read vendored catalog {path}: {e}"));
    let cat = parse_catalog(&text).expect("vendored catalog must parse");
    let issues = validate(&cat);
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "vendored catalog has errors: {errors:?}");
    assert!(!cat.models.is_empty());
    assert!(!cat.harnesses.is_empty());
    assert!(cat.agents.iter().any(|a| a.routable));
}
```

- [ ] **Step 3: Убедиться, что тесты падают**

Run: `cargo test -p arbiter-core --test catalog_validation`
Expected: FAIL — `validate`, `Severity`, `Issue` не существуют (ошибка компиляции).

- [ ] **Step 4: Реализовать `validate`**

Добавить в `arbiter-core/src/catalog/mod.rs` (после `parse_catalog`):

```rust
/// Issue severity: errors fail `catalog check`, warnings do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Fatal for `catalog check` (exit 1).
    Error,
    /// Non-fatal diagnostic.
    Warning,
}

/// A single validation finding (rule code V1..V7 + human message).
#[derive(Debug, Clone)]
pub struct Issue {
    /// Severity of the finding.
    pub severity: Severity,
    /// Rule code, e.g. `"V3"` (see design §4).
    pub code: &'static str,
    /// Human-readable description.
    pub message: String,
}

impl Issue {
    fn error(code: &'static str, message: String) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message,
        }
    }

    fn warning(code: &'static str, message: String) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message,
        }
    }
}

/// Validate a parsed catalog against rules V1-V7 (design §4).
/// Collects ALL issues — no short-circuit. V2+V3 together mirror
/// conformance Check 5 (missing/retired enrollment references).
pub fn validate(catalog: &Catalog) -> Vec<Issue> {
    let mut issues = Vec::new();

    // V7: unknown enum values (degrade-with-warning, design §3).
    for (name, model) in &catalog.models {
        if let ModelStatus::Other(s) = &model.status {
            issues.push(Issue::warning(
                "V7",
                format!("model '{name}' has unknown status '{s}'"),
            ));
        }
    }
    for (name, harness) in &catalog.harnesses {
        if let HarnessKind::Other(s) = &harness.kind {
            issues.push(Issue::warning(
                "V7",
                format!("harness '{name}' has unknown kind '{s}'"),
            ));
        }
    }

    let mut seen_ids = std::collections::HashSet::new();
    for agent in &catalog.agents {
        let id = agent.agent_id();

        // V4: duplicate agent_id.
        if !seen_ids.insert(id.clone()) {
            issues.push(Issue::error(
                "V4",
                format!("duplicate agent_id '{id}'"),
            ));
        }

        // V1 + V5: harness reference and plane consistency.
        match catalog.harnesses.get(&agent.harness) {
            None => issues.push(Issue::error(
                "V1",
                format!("agent '{id}' references undeclared harness '{}'", agent.harness),
            )),
            Some(harness) => {
                if agent.routable && !harness.routable {
                    issues.push(Issue::error(
                        "V5",
                        format!(
                            "agent '{id}' is routable but harness '{}' is not",
                            agent.harness
                        ),
                    ));
                }
            }
        }

        // V2 + V3 + V6: model reference and lifecycle.
        match catalog.models.get(&agent.model) {
            None => issues.push(Issue::error(
                "V2",
                format!("agent '{id}' references undeclared model '{}'", agent.model),
            )),
            Some(model) => match &model.status {
                ModelStatus::Retired => issues.push(Issue::error(
                    "V3",
                    format!("agent '{id}' references retired model '{}'", agent.model),
                )),
                ModelStatus::Deprecated => issues.push(Issue::warning(
                    "V6",
                    format!("agent '{id}' references deprecated model '{}'", agent.model),
                )),
                ModelStatus::Active | ModelStatus::Other(_) => {}
            },
        }
    }

    issues
}
```

- [ ] **Step 5: Прогнать тесты**

Run: `cargo test -p arbiter-core --test catalog_validation && cargo test -p arbiter-core catalog`
Expected: PASS (8 интеграционных + 6 модульных).

- [ ] **Step 6: fmt + clippy + коммит**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings
git add arbiter-core/src/catalog/mod.rs arbiter-core/tests/fixtures/catalog/ arbiter-core/tests/catalog_validation.rs
git commit -m "feat(catalog): validation rules V1-V7 + fixtures + vendored-catalog happy path

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Резолюция пути (`resolve_path` + `missing_file_error`)

**Files:**
- Modify: `arbiter-core/src/catalog/mod.rs`

**Interfaces:**
- Produces:
  - `pub enum CatalogSource { AtpCatalogEnv, XdgConfigHome, HomeDefault }`
  - `pub struct ResolvedPath { pub path: PathBuf, pub source: CatalogSource }`
  - `pub fn resolve_path<F: Fn(&str) -> Option<String>>(env: F, home: Option<&Path>) -> Result<ResolvedPath, CatalogError>`
  - `pub fn missing_file_error(resolved: &ResolvedPath) -> CatalogError` — `AtpCatalogEnv` → `EnvFileNotFound`, XDG/home → `NotConfigured` (спека §2: между слоями 2–3 fallback только по env, отсутствие файла = «не сконфигурировано»).

- [ ] **Step 1: Написать падающие тесты резолюции**

Добавить в `#[cfg(test)] mod tests` файла `arbiter-core/src/catalog/mod.rs`:

```rust
    use std::path::Path;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + '_ {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn resolve_prefers_atp_catalog_env() {
        let env = env_of(&[
            ("ATP_CATALOG", "/team/catalog.toml"),
            ("XDG_CONFIG_HOME", "/xdg"),
        ]);
        let r = resolve_path(env, Some(Path::new("/home/u"))).unwrap();
        assert_eq!(r.path, PathBuf::from("/team/catalog.toml"));
        assert!(matches!(r.source, CatalogSource::AtpCatalogEnv));
    }

    #[test]
    fn resolve_uses_xdg_config_home_when_set() {
        let env = env_of(&[("XDG_CONFIG_HOME", "/xdg")]);
        let r = resolve_path(env, Some(Path::new("/home/u"))).unwrap();
        assert_eq!(r.path, PathBuf::from("/xdg/atp/agents-catalog.toml"));
        assert!(matches!(r.source, CatalogSource::XdgConfigHome));
    }

    #[test]
    fn resolve_falls_back_to_home_config() {
        let env = env_of(&[]);
        let r = resolve_path(env, Some(Path::new("/home/u"))).unwrap();
        assert_eq!(
            r.path,
            PathBuf::from("/home/u/.config/atp/agents-catalog.toml")
        );
        assert!(matches!(r.source, CatalogSource::HomeDefault));
    }

    #[test]
    fn empty_env_values_are_treated_as_unset() {
        let env = env_of(&[("ATP_CATALOG", ""), ("XDG_CONFIG_HOME", "")]);
        let r = resolve_path(env, Some(Path::new("/home/u"))).unwrap();
        assert!(matches!(r.source, CatalogSource::HomeDefault));
    }

    #[test]
    fn resolve_fails_loud_when_nothing_configured() {
        let env = env_of(&[]);
        let err = resolve_path(env, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("$ATP_CATALOG"), "hint $ATP_CATALOG: {msg}");
        assert!(
            msg.contains("~/.config/atp/agents-catalog.toml"),
            "hint XDG path: {msg}"
        );
    }

    #[test]
    fn missing_file_error_depends_on_source() {
        let env_resolved = ResolvedPath {
            path: PathBuf::from("/team/catalog.toml"),
            source: CatalogSource::AtpCatalogEnv,
        };
        let msg = missing_file_error(&env_resolved).to_string();
        assert!(msg.contains("/team/catalog.toml"));
        assert!(msg.contains("$ATP_CATALOG"));

        let xdg_resolved = ResolvedPath {
            path: PathBuf::from("/xdg/atp/agents-catalog.toml"),
            source: CatalogSource::XdgConfigHome,
        };
        assert!(matches!(
            missing_file_error(&xdg_resolved),
            CatalogError::NotConfigured
        ));
    }
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test -p arbiter-core catalog`
Expected: FAIL — `resolve_path`, `ResolvedPath`, `CatalogSource`, `missing_file_error` не существуют (ошибка компиляции).

- [ ] **Step 3: Реализовать резолюцию**

Добавить в `arbiter-core/src/catalog/mod.rs` (после `CatalogError`, до типов плоскостей; `use std::path::Path;` — в шапку):

```rust
/// Which resolution layer produced the path (ADR-003b D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSource {
    /// Layer 1: explicit `$ATP_CATALOG` path (no fallback below it).
    AtpCatalogEnv,
    /// Layer 2: `$XDG_CONFIG_HOME/atp/agents-catalog.toml`.
    XdgConfigHome,
    /// Layer 3: `~/.config/atp/agents-catalog.toml`.
    HomeDefault,
}

/// A resolved candidate path plus the layer that produced it.
#[derive(Debug, Clone)]
pub struct ResolvedPath {
    /// The single candidate path (existence NOT checked here — no I/O).
    pub path: PathBuf,
    /// Resolution layer, used to pick the right missing-file error.
    pub source: CatalogSource,
}

/// Resolve the catalog path per ADR-003b D2. Pure: env access and home
/// dir are injected. Returns exactly one candidate; the caller checks
/// file existence and maps a miss via [`missing_file_error`].
/// Empty env values are treated as unset. Layers 2-3 are one XDG layer
/// with a default: the choice depends only on whether `XDG_CONFIG_HOME`
/// is set, never on file existence.
pub fn resolve_path<F>(env: F, home: Option<&Path>) -> Result<ResolvedPath, CatalogError>
where
    F: Fn(&str) -> Option<String>,
{
    let non_empty = |key: &str| env(key).filter(|v| !v.is_empty());

    if let Some(explicit) = non_empty(CATALOG_ENV_VAR) {
        return Ok(ResolvedPath {
            path: PathBuf::from(explicit),
            source: CatalogSource::AtpCatalogEnv,
        });
    }
    if let Some(xdg) = non_empty("XDG_CONFIG_HOME") {
        return Ok(ResolvedPath {
            path: PathBuf::from(xdg).join(XDG_SUBPATH),
            source: CatalogSource::XdgConfigHome,
        });
    }
    if let Some(home) = home {
        return Ok(ResolvedPath {
            path: home.join(".config").join(XDG_SUBPATH),
            source: CatalogSource::HomeDefault,
        });
    }
    Err(CatalogError::NotConfigured)
}

/// Map a missing file at the resolved path to the right error:
/// an explicit `$ATP_CATALOG` miss names the path (no silent fallback);
/// an XDG-layer miss means the catalog simply is not configured.
pub fn missing_file_error(resolved: &ResolvedPath) -> CatalogError {
    match resolved.source {
        CatalogSource::AtpCatalogEnv => CatalogError::EnvFileNotFound {
            path: resolved.path.clone(),
        },
        CatalogSource::XdgConfigHome | CatalogSource::HomeDefault => {
            CatalogError::NotConfigured
        }
    }
}
```

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test -p arbiter-core catalog`
Expected: PASS (12 модульных).

- [ ] **Step 5: fmt + clippy + коммит**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings
git add arbiter-core/src/catalog/mod.rs
git commit -m "feat(catalog): pure path resolution \$ATP_CATALOG -> XDG, fail-loud

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: CLI `arbiter-cli catalog path|check|list`

**Files:**
- Modify: `arbiter-cli/src/main.rs` (usage-блок + диспетч + функции команды)
- Create: `arbiter-cli/tests/catalog_cli.rs`

**Interfaces:**
- Consumes: `arbiter_core::catalog::{parse_catalog, validate, resolve_path, missing_file_error, Catalog, Severity, CatalogError}`.
- Produces: сабкоманды `catalog path` / `catalog check` / `catalog list`; exit 0 = ок (warnings допустимы), exit 1 = ошибки валидации или конфиг не найден/не читается.

- [ ] **Step 1: Написать падающие CLI-тесты**

`arbiter-cli/tests/catalog_cli.rs`:

```rust
//! Smoke tests for `arbiter-cli catalog` subcommands: exit codes and
//! fail-loud messages (design §6). Uses the compiled binary directly.

use std::process::{Command, Output};

fn run_catalog(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_arbiter-cli"));
    cmd.arg("catalog").args(args);
    // Isolate from the developer's real environment.
    cmd.env_remove("ATP_CATALOG")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("HOME");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run arbiter-cli")
}

fn vendored_catalog() -> String {
    format!(
        "{}/../config/agents-catalog.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn fixture(name: &str) -> String {
    format!(
        "{}/../arbiter-core/tests/fixtures/catalog/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn check_passes_on_vendored_catalog() {
    let out = run_catalog(&["check"], &[("ATP_CATALOG", &vendored_catalog())]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("catalog OK"));
}

#[test]
fn check_fails_on_retired_reference() {
    let out = run_catalog(&["check"], &[("ATP_CATALOG", &fixture("retired_ref.toml"))]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("V3"));
}

#[test]
fn check_fails_loud_without_any_config() {
    let out = run_catalog(&["check"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("$ATP_CATALOG"), "stderr: {stderr}");
}

#[test]
fn check_fails_when_env_path_missing() {
    let out = run_catalog(&["check"], &[("ATP_CATALOG", "/nonexistent/cat.toml")]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("/nonexistent/cat.toml"), "stderr: {stderr}");
}

#[test]
fn path_prints_resolved_path() {
    let cat = vendored_catalog();
    let out = run_catalog(&["path"], &[("ATP_CATALOG", &cat)]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), cat);
}

#[test]
fn path_exits_nonzero_when_file_missing() {
    let out = run_catalog(&["path"], &[("ATP_CATALOG", "/nonexistent/cat.toml")]);
    assert_eq!(out.status.code(), Some(1));
    // Path is still printed (useful for debugging), error goes to stderr.
    assert!(String::from_utf8_lossy(&out.stdout).contains("/nonexistent/cat.toml"));
}

#[test]
fn list_prints_agents_table() {
    let out = run_catalog(&["list"], &[("ATP_CATALOG", &vendored_catalog())]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("AGENT_ID"));
    assert!(stdout.contains("claude_code@claude-sonnet-4-6"));
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let out = run_catalog(&["frobnicate"], &[]);
    assert_eq!(out.status.code(), Some(1));
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test -p arbiter-cli --test catalog_cli`
Expected: FAIL — сабкоманда `catalog` не существует, все тесты падают на exit code / выводе usage.

- [ ] **Step 3: Реализовать сабкоманду**

В `arbiter-cli/src/main.rs`:

1. В шапку импортов добавить:

```rust
use std::path::PathBuf;

use arbiter_core::catalog::{
    self, Catalog, CatalogError, Severity,
};
```

2. Usage-блок в `main()` дополнить строками:

```rust
        eprintln!("  arbiter-cli catalog path    Print resolved catalog path");
        eprintln!("  arbiter-cli catalog check   Validate the user-config catalog");
        eprintln!("  arbiter-cli catalog list    List enrolled agents");
```

3. В `main()` после ветки `bench` добавить диспетч (по образцу hand-rolled `bench`, clap не вводим — design §6):

```rust
    if args[1] == "catalog" {
        std::process::exit(run_catalog(args.get(2).map(String::as_str)));
    }
```

4. Функции команды (в конец файла, перед `main`):

```rust
// ---------------------------------------------------------------------------
// catalog subcommand (design: docs/2026-07-05-catalog-loader-design.md §6)
// ---------------------------------------------------------------------------

/// Resolve the catalog path from the real environment (the only place
/// env/home are read; core stays pure).
fn resolve_from_real_env() -> Result<catalog::ResolvedPath, CatalogError> {
    let home = std::env::var("HOME").ok().map(PathBuf::from);
    catalog::resolve_path(|key| std::env::var(key).ok(), home.as_deref())
}

/// Resolve + read + parse the user-config catalog (fail-loud).
fn load_catalog() -> Result<(PathBuf, Catalog), String> {
    let resolved = resolve_from_real_env().map_err(|e| e.to_string())?;
    if !resolved.path.exists() {
        return Err(catalog::missing_file_error(&resolved).to_string());
    }
    let text = std::fs::read_to_string(&resolved.path)
        .map_err(|e| format!("failed to read {}: {e}", resolved.path.display()))?;
    let cat = catalog::parse_catalog(&text).map_err(|e| e.to_string())?;
    Ok((resolved.path, cat))
}

/// `catalog path|check|list` dispatch; returns the process exit code.
fn run_catalog(subcommand: Option<&str>) -> i32 {
    match subcommand {
        Some("path") => catalog_path(),
        Some("check") => catalog_check(),
        Some("list") => catalog_list(),
        other => {
            eprintln!(
                "unknown catalog subcommand {:?}; expected path|check|list",
                other.unwrap_or("")
            );
            1
        }
    }
}

/// Print the resolved candidate path; exit 1 if it cannot be resolved or
/// the file does not exist (fail-loud surface, design §6).
fn catalog_path() -> i32 {
    match resolve_from_real_env() {
        Ok(resolved) => {
            println!("{}", resolved.path.display());
            if resolved.path.exists() {
                0
            } else {
                eprintln!("{}", catalog::missing_file_error(&resolved));
                1
            }
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// Validate the catalog: print all issues + summary; exit 1 on errors.
fn catalog_check() -> i32 {
    let (path, cat) = match load_catalog() {
        Ok(loaded) => loaded,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };
    let issues = arbiter_core::catalog::validate(&cat);
    let mut errors = 0usize;
    let mut warnings = 0usize;
    for issue in &issues {
        match issue.severity {
            Severity::Error => {
                errors += 1;
                println!("ERROR {}: {}", issue.code, issue.message);
            }
            Severity::Warning => {
                warnings += 1;
                println!("WARN  {}: {}", issue.code, issue.message);
            }
        }
    }
    if errors > 0 {
        println!(
            "catalog INVALID: {} ({errors} errors, {warnings} warnings)",
            path.display()
        );
        1
    } else {
        println!(
            "catalog OK: {} ({} models, {} harnesses, {} agents, {warnings} warnings)",
            path.display(),
            cat.models.len(),
            cat.harnesses.len(),
            cat.agents.len()
        );
        0
    }
}

/// Print the enrollment table; same exit semantics as `check`.
fn catalog_list() -> i32 {
    let (_, cat) = match load_catalog() {
        Ok(loaded) => loaded,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };
    println!(
        "{:<45} {:<7} {:<9} {}",
        "AGENT_ID", "TESTED", "ROUTABLE", "MODEL STATUS"
    );
    for agent in &cat.agents {
        let status = cat
            .models
            .get(&agent.model)
            .map(|m| m.status.to_string())
            .unwrap_or_else(|| "missing!".to_string());
        println!(
            "{:<45} {:<7} {:<9} {}",
            agent.agent_id(),
            agent.tested,
            agent.routable,
            status
        );
    }
    let has_errors = arbiter_core::catalog::validate(&cat)
        .iter()
        .any(|i| i.severity == Severity::Error);
    if has_errors {
        eprintln!("(catalog has validation errors — run `arbiter-cli catalog check`)");
        1
    } else {
        0
    }
}
```

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test -p arbiter-cli --test catalog_cli`
Expected: PASS (8 тестов).

- [ ] **Step 5: fmt + clippy + коммит**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings
git add arbiter-cli/src/main.rs arbiter-cli/tests/catalog_cli.rs
git commit -m "feat(cli): catalog path|check|list subcommands (fail-loud user-config loader)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Полный прогон, документация, TODO

**Files:**
- Modify: `CLAUDE.md` (структура проекта: `catalog/` в arbiter-core, `catalog_cli.rs`; упоминание CLI-команды)
- Modify: `TODO.md` (новая запись о выполненной задаче с хешем коммита — по «Правилам ведения»)

**Interfaces:**
- Consumes: всё из Task 1–4.
- Produces: зелёный полный прогон, синхронизированная документация.

- [ ] **Step 1: Полный прогон workspace**

```bash
cargo fmt --all -- --check && cargo clippy --workspace -- -D warnings && cargo test --workspace
```

Expected: всё зелёное, включая существующие ~290 unit + integration + golden тесты (регрессий нет — runtime не трогали).

- [ ] **Step 2: Обновить CLAUDE.md**

В блоке Project Structure:
- под `arbiter-core/src/` после `error.rs` добавить строку:
  `│       ├── catalog/mod.rs        # User-config agents-catalog loader (ADR-ECO-003b): parse, validate V1-V7, resolve $ATP_CATALOG→XDG`
- под `arbiter-cli/` добавить `│   └── tests/catalog_cli.rs      # CLI smoke tests for catalog subcommands`

В секции «Build & Test Commands» после `arbiter-cli -- bench` добавить:

```bash
# Validate the user-config agents catalog ($ATP_CATALOG → ~/.config/atp/agents-catalog.toml)
cargo run --release --bin arbiter-cli -- catalog check
```

Примечание к правилу №1 (arbiter-core is a library): дополнить фразой, что `catalog` — pure-модуль (env/файлы инжектируются, I/O у вызывающего) — исключение НЕ требуется.

- [ ] **Step 3: Обновить TODO.md**

В секцию «Активные задачи» добавить:

```markdown
### ADR-ECO-003b: Rust-загрузчик user-config каталога — ✅ закрыт

- [x] `arbiter_core::catalog`: parse (3 плоскости, degrade-with-warning для
  незнакомых enum) + validate V1–V7 (V2+V3 зеркалят conformance Check 5) +
  `resolve_path` ($ATP_CATALOG → XDG `atp/`, fail-loud, без bundled-дефолта)
- [x] `arbiter-cli catalog path|check|list` — CLI-поверхность fail-loud
- [x] Дизайн: `docs/2026-07-05-catalog-loader-design.md` (XDG-namespace `atp/`
  зафиксирован — закрывает открытый пункт ADR-003b)
- Коммит: `<hash последнего коммита Task 4>`
```

- [ ] **Step 4: Финальный коммит**

```bash
git add CLAUDE.md TODO.md
git commit -m "docs: catalog loader in project structure, TODO entry (ADR-ECO-003b)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Out of scope (из спеки §1, не делать)

- Интеграция в arbiter-mcp (валидация agents.toml против каталога при старте).
- `models init/discover/update` — ATP CLI.
- Кросс-языковой conformance-тест трёх загрузчиков (фикстуры уже оформлены пошарибельно: одна-фикстура-одно-правило).
