# 配置中支持包版本约束 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `pip-mirror` 的 `packages` 配置中支持 PEP 440 版本约束，如 `"geopandas[all]==5.0.0"` 和 `"numpy>=1.26.0,<2.0"`。

**Architecture:** 新增一个中立层解析函数 `parse_package_ref` 把字符串拆成（包名、extras、版本约束）；版本约束通过 `PlanParams.version_specs` 传到 resolver，在 `collect_top_versions` 中用现有 `spec_to_range` 过滤候选版本后再取 top-N；配置加载阶段做严格校验，无匹配版本时在 plan 阶段报错。

**Tech Stack:** Rust, pep440_rs, pubgrub, tokio, cargo test, uv

---

## 文件结构

| 文件 | 职责 |
|------|------|
| `src/filters.rs` | 新增 `ParsedPackageRef` 和 `parse_package_ref()`，负责把配置字符串拆成 name/extras/version_spec |
| `src/resolver/pubgrub.rs` | 修正 `extract_extras()` 正确处理 `]` 后内容；把 `collect_pkg_extras()` 改名为 `collect_pkg_refs()`；新增严格版本约束校验函数 |
| `src/resolver/plan/mod.rs` | `PlanParams` 新增 `version_specs`；`collect_top_versions` 用版本约束过滤版本列表 |
| `src/sync/phases/plan.rs` | 构造 `version_specs` 并传入 `PlanParams`；`no_deps` 路径也传递 |
| `src/sync/plan.rs` | `build_top_only_plan` 接收 `version_specs` 并过滤版本 |
| `src/config/validator.rs` | 新增 `ConfigError::InvalidVersionSpec` / `DuplicateVersionSpec`；在 `validate()` 中校验每个 `PackageSpec::Name` |
| `src/resolver/error.rs` | 新增 `ResolveError::NoMatchingVersion` |
| `tests/resolver_tests.rs` | 补充 `parse_package_ref` 和 `collect_pkg_refs` 单元测试 |
| `tests/integration_tests.rs` | 更新 `collect_pkg_extras` → `collect_pkg_refs` 的调用 |
| `tests/config_tests.rs` | 补充配置校验测试 |
| `.github/workflows/e2e.yml` | 加入真实版本约束案例 `certifi==2024.6.2` 及客户端验证 |

---

### Task 1: 在 `src/filters.rs` 添加 `ParsedPackageRef` 和 `parse_package_ref`

**Files:**
- Modify: `src/filters.rs:1-3`
- Test: `tests/resolver_tests.rs`

- [ ] **Step 1: 添加 `ParsedPackageRef` 结构体并引入 `HashMap`**

把 `src/filters.rs` 的 imports 改为：

```rust
use std::collections::{HashMap, HashSet};

use crate::resolver::types::TargetEnv;
```

在文件末尾（`select_files_for_version` 之后）追加：

```rust
/// 解析后的包引用，包含归一化名称、extras 集合和可选版本约束。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPackageRef {
    pub name: String,
    pub extras: HashSet<String>,
    pub version_spec: Option<String>,
}

/// 解析配置中的包引用字符串。
///
/// 支持格式：
/// - `numpy`
/// - `markitdown[pptx,docx]`
/// - `numpy==2.5.0`
/// - `geopandas[all]==5.0.0`
/// - `numpy>=1.20,<2.0`
///
/// 版本操作符两侧不允许空格。
pub fn parse_package_ref(raw: &str) -> Result<ParsedPackageRef, String> {
    if raw.is_empty() {
        return Err("包引用不能为空".to_string());
    }

    // 先提取 [extras]
    let (name_part, extras) = if let Some((name, rest)) = raw.split_once('[') {
        let (extras_str, after_bracket) = rest
            .split_once(']')
            .ok_or_else(|| "缺少右括号 ']'".to_string())?;
        if !after_bracket.is_empty() && !after_bracket.starts_with(|c: char| {
            matches!(c, '>' | '<' | '=' | '!' | '~')
        }) {
            return Err(format!("']' 后存在非版本约束内容: {after_bracket}"));
        }
        let extras: HashSet<String> = extras_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        (name, extras)
    } else {
        (raw, HashSet::new())
    };

    // 在剩余部分找版本约束起点：第一个非包名字符
    let (name, version_spec) = match name_part
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.' && c != '-')
    {
        None => (name_part, None),
        Some(idx) => {
            let (name, spec) = name_part.split_at(idx);
            if name.is_empty() {
                return Err(format!("包名缺失: {raw}"));
            }
            (name, Some(spec.trim().to_string()))
        }
    };

    if let Some(ref spec) = version_spec {
        if spec.contains(' ') {
            return Err("版本约束中不允许空格".to_string());
        }
        if spec.is_empty() {
            return Err("版本约束不能为空".to_string());
        }
    }

    Ok(ParsedPackageRef {
        name: normalize_package_name(name),
        extras,
        version_spec,
    })
}
```

- [ ] **Step 2: 编译检查**

Run: `cargo check`
Expected: 通过，无错误。

- [ ] **Step 3: 提交**

```bash
git add src/filters.rs
git commit -m "feat(filters): add parse_package_ref for name/extras/version_spec"
```

---

### Task 2: 在 `src/resolver/pubgrub.rs` 修正 extras 提取并新增 `collect_pkg_refs`

**Files:**
- Modify: `src/resolver/pubgrub.rs:1-39`
- Test: `tests/resolver_tests.rs`

- [ ] **Step 1: 修改 imports 并替换 `extract_extras` / `collect_pkg_extras`**

把 `src/resolver/pubgrub.rs` 顶部改为：

```rust
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use pep440_rs::Version;
use pubgrub::Range;

use crate::filters::{normalize_package_name, parse_package_ref, ParsedPackageRef};
```

替换 `extract_extras` 和 `collect_pkg_extras`：

```rust
pub fn extract_extras(package_ref: &str) -> (String, HashSet<String>) {
    match parse_package_ref(package_ref) {
        Ok(parsed) => (parsed.name, parsed.extras),
        Err(_) => (package_ref.to_string(), HashSet::new()),
    }
}

pub fn collect_pkg_refs(
    packages: &[String],
) -> HashMap<String, ParsedPackageRef> {
    let mut pkg_refs = HashMap::new();
    for pkg_ref in packages {
        if let Ok(parsed) = parse_package_ref(pkg_ref) {
            pkg_refs.insert(parsed.name.clone(), parsed);
        }
    }
    pkg_refs
}
```

- [ ] **Step 2: 新增严格版本约束校验函数**

在 `spec_to_range` 之后追加：

```rust
/// 严格校验用户指定的版本约束字符串。
///
/// 与 `spec_to_range` 不同：遇到任何无效操作符或无效版本号都会返回 Err，
/// 而不是静默跳过。用于配置加载阶段，避免用户写错约束被当作无约束处理。
pub fn validate_version_spec(spec: &str) -> Result<(), String> {
    if spec.is_empty() {
        return Err("版本约束不能为空".to_string());
    }
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err("版本约束中有多余逗号".to_string());
        }
        let Some((op, ver_str)) = split_operator(part) else {
            return Err(format!("无效版本操作符: {part}"));
        };
        let ver_str = ver_str.trim();
        if ver_str.is_empty() {
            return Err(format!("{op} 后缺少版本号"));
        }
        if Version::from_str(ver_str).is_err() {
            return Err(format!("无效版本号: {ver_str}"));
        }
    }
    Ok(())
}
```

- [ ] **Step 3: 编译检查**

Run: `cargo check`
Expected: 通过，无错误。

- [ ] **Step 4: 提交**

```bash
git add src/resolver/pubgrub.rs
git commit -m "feat(resolver): collect_pkg_refs and strict version spec validation"
```

---

### Task 3: 修改 `src/resolver/plan/mod.rs` 添加 `version_specs`

**Files:**
- Modify: `src/resolver/plan/mod.rs:1-192`
- Test: `tests/integration_tests.rs`

- [ ] **Step 1: 修改 imports 和 `PlanParams`**

在 `src/resolver/plan/mod.rs` 中：

```rust
use crate::filters::spec_to_range;  // 新增
use super::pubgrub::{bare_name, collect_pkg_refs};  // 改为 collect_pkg_refs
```

把 `PlanParams` 改为：

```rust
pub struct PlanParams<'a> {
    pub top_packages: &'a [String],
    pub pypi_urls: &'a [String],
    pub top_versions_per_package: usize,
    pub adjacent_versions_per_side: usize,
    pub allow_prerelease: bool,
    pub include_source: bool,
    pub linux_max_glibc: &'a str,
    pub resolve_workers: usize,
    pub metadata_workers: usize,
    pub targets: Vec<TargetEnv>,
    pub version_specs: &'a HashMap<String, Option<String>>,  // 新增
}
```

- [ ] **Step 2: 修改 `build_dependency_plan_inner` 用 `collect_pkg_refs` 取 extras**

替换：

```rust
let top_versions = collect_top_versions(params, &cache).await?;
let pkg_extras = collect_pkg_extras(params.top_packages);
```

为：

```rust
let top_versions = collect_top_versions(params, &cache).await?;
let pkg_refs = collect_pkg_refs(params.top_packages);
let pkg_extras: HashMap<String, HashSet<String>> = pkg_refs
    .iter()
    .map(|(name, parsed)| (name.clone(), parsed.extras.clone()))
    .collect();
```

- [ ] **Step 3: 修改 `collect_top_versions` 用版本约束过滤**

替换整个 `collect_top_versions` 函数为：

```rust
async fn collect_top_versions(
    params: &PlanParams<'_>,
    cache: &MetadataCache,
) -> Result<HashMap<String, Vec<Version>>, ResolveError> {
    let results = stream::iter(params.top_packages.iter())
        .map(|package_ref| async move {
            let package = bare_name(package_ref);
            let all_versions = cache.get_all_versions(&package).await?;
            let candidates = if let Some(Some(spec)) =
                params.version_specs.get(&package)
            {
                let range = spec_to_range(spec);
                let filtered: Vec<_> = all_versions
                    .into_iter()
                    .filter(|v| range.contains(v))
                    .collect();
                if filtered.is_empty() {
                    return Err(ResolveError::NoMatchingVersion {
                        package: package.clone(),
                        spec: spec.clone(),
                    });
                }
                filtered
            } else {
                all_versions
            };
            let selected = select_top_versions(
                candidates,
                params.top_versions_per_package,
                params.allow_prerelease,
            );
            debug!("顶层包 {}: 选定 {} 个版本", package, selected.len());
            Ok::<_, ResolveError>((package, selected))
        })
        .buffer_unordered(params.resolve_workers)
        .collect::<Vec<_>>()
        .await;
    let mut top_versions =
        results.into_iter().collect::<Result<Vec<_>, _>>()?;
    top_versions.sort_by(|(l, _), (r, _)| l.cmp(r));
    Ok(top_versions.into_iter().collect())
}
```

- [ ] **Step 4: 编译检查**

Run: `cargo check`
Expected: 通过。

- [ ] **Step 5: 提交**

```bash
git add src/resolver/plan/mod.rs
git commit -m "feat(resolver): filter top versions by user version specs"
```

---

### Task 4: 修改 `src/resolver/error.rs` 添加 `NoMatchingVersion`

**Files:**
- Modify: `src/resolver/error.rs:1-57`

- [ ] **Step 1: 新增错误变体**

把 `ResolveError` 改为：

```rust
#[derive(Debug, Clone)]
pub enum ResolveError {
    Metadata(MetadataError),
    Marker(MarkerError),
    InvalidRequiresPython {
        package: String,
        version: Version,
        spec: String,
        detail: String,
    },
    NoSolution {
        package: String,
        version: Version,
        target: String,
        detail: String,
    },
    NoMatchingVersion {
        package: String,
        spec: String,
    },
    Config(String),
}
```

在 `Display` 实现中追加分支：

```rust
ResolveError::NoMatchingVersion { package, spec } => {
    write!(f, "包 {package} 在 PyPI 上找不到匹配版本约束 {spec} 的版本")
}
```

- [ ] **Step 2: 编译检查**

Run: `cargo check`
Expected: 通过。

- [ ] **Step 3: 提交**

```bash
git add src/resolver/error.rs
git commit -m "feat(resolver): add NoMatchingVersion error variant"
```

---

### Task 5: 修改 `src/sync/phases/plan.rs` 构造并传递 `version_specs`

**Files:**
- Modify: `src/sync/phases/plan.rs:1-74`

- [ ] **Step 1: 引入 `collect_pkg_refs` 并构造 `version_specs`**

把 `src/sync/phases/plan.rs` 中 `use crate::resolver::plan::{...}` 改为：

```rust
use crate::resolver::plan::{
    DependencyPlan, PlanParams, build_dependency_plan,
};
use crate::resolver::pubgrub::collect_pkg_refs;
```

在 `PlanPhase::run` 的 `let (mut name_pkgs, url_pkgs) = ...` 之后插入：

```rust
let pkg_refs = collect_pkg_refs(&name_pkgs);
let version_specs: std::collections::HashMap<String, Option<String>> =
    pkg_refs
        .iter()
        .map(|(name, parsed)| (name.clone(), parsed.version_spec.clone()))
        .collect();
```

并把 `version_specs` 传给 `build_plan`：

```rust
let mut plan =
    build_plan(config, client, &name_pkgs, &version_specs, no_deps, progress)
        .await?;
```

- [ ] **Step 2: 修改 `build_plan` 签名和 `PlanParams` 构造**

把 `build_plan` 改为：

```rust
async fn build_plan(
    config: &Config,
    client: &HttpClient,
    name_pkgs: &[String],
    version_specs: &std::collections::HashMap<String, Option<String>>,
    no_deps: bool,
    progress: Option<ProgressHandle>,
) -> Result<DependencyPlan, ResolveError> {
    if no_deps {
        return plan::build_top_only_plan(
            config,
            client,
            name_pkgs,
            version_specs,
        )
        .await;
    }
    let params = PlanParams {
        top_packages: name_pkgs,
        pypi_urls: &config.effective_mirrors(),
        top_versions_per_package: config.top_versions_per_package,
        adjacent_versions_per_side: config.adjacent_versions_per_side,
        allow_prerelease: config.allow_prerelease,
        include_source: config.include_source,
        linux_max_glibc: &config.linux_max_glibc,
        resolve_workers: config.resolve_workers,
        metadata_workers: config.metadata_workers,
        targets: crate::resolver::types::TargetEnv::from_specs(&config.targets),
        version_specs,
    };
    build_dependency_plan(&params, client, progress).await
}
```

- [ ] **Step 3: 编译检查**

Run: `cargo check`
Expected: 通过。

- [ ] **Step 4: 提交**

```bash
git add src/sync/phases/plan.rs
git commit -m "feat(sync): pass version_specs into PlanParams"
```

---

### Task 6: 修改 `src/sync/plan.rs` 的 `build_top_only_plan` 处理 `version_specs`

**Files:**
- Modify: `src/sync/plan.rs:1-94`

- [ ] **Step 1: 引入 `spec_to_range` 并修改函数签名**

在 `src/sync/plan.rs` imports 中新增：

```rust
use crate::filters::spec_to_range;
```

把 `build_top_only_plan` 签名改为：

```rust
pub async fn build_top_only_plan(
    config: &crate::config::Config,
    client: &HttpClient,
    pkgs: &[String],
    version_specs: &std::collections::HashMap<String, Option<String>>,
) -> Result<DependencyPlan, ResolveError> {
```

- [ ] **Step 2: 在循环中用版本约束过滤**

替换循环中的版本选择逻辑：

```rust
for pkg in pkgs {
    let package = bare_name(pkg);
    let all_versions = cache.get_all_versions(&package).await?;
    let candidates = if let Some(Some(spec)) = version_specs.get(&package) {
        let range = spec_to_range(spec);
        let filtered: Vec<_> =
            all_versions.into_iter().filter(|v| range.contains(v)).collect();
        if filtered.is_empty() {
            return Err(ResolveError::NoMatchingVersion {
                package: package.clone(),
                spec: spec.clone(),
            });
        }
        filtered
    } else {
        all_versions
    };
    let selected_versions = select_top_versions(
        candidates,
        config.top_versions_per_package,
        config.allow_prerelease,
    );
    // ... 以下不变 ...
}
```

- [ ] **Step 3: 编译检查**

Run: `cargo check`
Expected: 通过。

- [ ] **Step 4: 提交**

```bash
git add src/sync/plan.rs
git commit -m "feat(sync): apply version specs in no-deps top-only plan"
```

---

### Task 7: 修改 `src/config/validator.rs` 添加配置校验

**Files:**
- Modify: `src/config/validator.rs:1-160`

- [ ] **Step 1: 引入 `parse_package_ref` 和 `HashMap`**

把 imports 改为：

```rust
use std::collections::HashMap;
use std::fmt;

use url::Url;

use super::{Config, PackageSpec, PackageUrlSpec};
use crate::filters::parse_package_ref;
use crate::redact::redact_url_for_display;
```

- [ ] **Step 2: 扩展 `ConfigError`**

在 `ConfigError` 枚举中新增：

```rust
/// 包引用中的版本约束格式无效。
InvalidVersionSpec { package: String, raw: String, reason: String },
/// 同一包名在 packages 中出现多次且带有版本约束。
DuplicateVersionSpec { package: String },
```

在 `fmt::Debug` 中新增：

```rust
ConfigError::InvalidVersionSpec { package, raw, reason } => f
    .debug_struct("InvalidVersionSpec")
    .field("package", package)
    .field("raw", raw)
    .field("reason", reason)
    .finish(),
ConfigError::DuplicateVersionSpec { package } => f
    .debug_struct("DuplicateVersionSpec")
    .field("package", package)
    .finish(),
```

在 `fmt::Display` 中新增：

```rust
ConfigError::InvalidVersionSpec { package, raw, reason } => {
    let safe = redact_url_for_display(raw);
    if package.is_empty() {
        write!(f, "包引用 `{safe}` 的版本约束无效: {reason}")
    } else {
        write!(f, "包 `{package}` 的版本约束 `{safe}` 无效: {reason}")
    }
}
ConfigError::DuplicateVersionSpec { package } => {
    write!(f, "包 `{package}` 在 packages 中重复出现且带有版本约束")
}
```

- [ ] **Step 3: 在 `validate()` 中调用新的校验函数**

在 `validate()` 中，现有 `Self::validate_mirrors(...)` 之后插入：

```rust
Self::validate_package_refs(&config.packages)?;
```

在 `impl ConfigValidator` 中新增函数：

```rust
fn validate_package_refs(
    packages: &[PackageSpec],
) -> Result<(), ConfigError> {
    let mut seen: HashMap<String, Option<String>> = HashMap::new();
    for spec in packages.iter().filter_map(|s| s.as_name()) {
        let parsed = parse_package_ref(spec).map_err(|reason| {
            ConfigError::InvalidVersionSpec {
                package: String::new(),
                raw: spec.to_string(),
                reason,
            }
        })?;

        if let Some(version_spec) = &parsed.version_spec {
            crate::resolver::pubgrub::validate_version_spec(version_spec)
                .map_err(|reason| ConfigError::InvalidVersionSpec {
                    package: parsed.name.clone(),
                    raw: spec.to_string(),
                    reason,
                })?;
        }

        if let Some(existing) = seen.get(&parsed.name) {
            if existing.is_some() || parsed.version_spec.is_some() {
                return Err(ConfigError::DuplicateVersionSpec {
                    package: parsed.name,
                });
            }
        }
        seen.insert(parsed.name, parsed.version_spec);
    }
    Ok(())
}
```

- [ ] **Step 4: 编译检查**

Run: `cargo check`
Expected: 通过。

- [ ] **Step 5: 提交**

```bash
git add src/config/validator.rs
git commit -m "feat(config): validate version specs in packages config"
```

---

### Task 8: 更新 `tests/resolver_tests.rs` 补充解析测试

**Files:**
- Modify: `tests/resolver_tests.rs`

- [ ] **Step 1: 在文件顶部新增 imports**

```rust
use std::collections::HashSet;

use pep440_rs::Version;
use pip_mirror::filters::{ParsedPackageRef, parse_package_ref};
use pip_mirror::resolver::pubgrub;
use pip_mirror::resolver::types::TargetEnv;
```

- [ ] **Step 2: 添加 `parse_package_ref` 测试**

在 `test_extract_extras_no_brackets` 之前或之后插入：

```rust
#[test]
fn test_parse_package_ref_bare_name() {
    let parsed = parse_package_ref("numpy").unwrap();
    assert_eq!(
        parsed,
        ParsedPackageRef {
            name: "numpy".to_string(),
            extras: HashSet::new(),
            version_spec: None,
        }
    );
}

#[test]
fn test_parse_package_ref_extras_only() {
    let parsed = parse_package_ref("markitdown[pptx,docx]").unwrap();
    assert_eq!(
        parsed,
        ParsedPackageRef {
            name: "markitdown".to_string(),
            extras: HashSet::from([
                "pptx".to_string(),
                "docx".to_string(),
            ]),
            version_spec: None,
        }
    );
}

#[test]
fn test_parse_package_ref_version_only() {
    let parsed = parse_package_ref("numpy==2.5.0").unwrap();
    assert_eq!(
        parsed,
        ParsedPackageRef {
            name: "numpy".to_string(),
            extras: HashSet::new(),
            version_spec: Some("==2.5.0".to_string()),
        }
    );
}

#[test]
fn test_parse_package_ref_extras_and_version() {
    let parsed = parse_package_ref("geopandas[all]==5.0.0").unwrap();
    assert_eq!(
        parsed,
        ParsedPackageRef {
            name: "geopandas".to_string(),
            extras: HashSet::from(["all".to_string()]),
            version_spec: Some("==5.0.0".to_string()),
        }
    );
}

#[test]
fn test_parse_package_ref_range() {
    let parsed = parse_package_ref("numpy>=1.20,<2.0").unwrap();
    assert_eq!(
        parsed,
        ParsedPackageRef {
            name: "numpy".to_string(),
            extras: HashSet::new(),
            version_spec: Some(">=1.20,<2.0".to_string()),
        }
    );
}

#[test]
fn test_parse_package_ref_rejects_space() {
    assert!(parse_package_ref("numpy == 2.5.0").is_err());
}

#[test]
fn test_parse_package_ref_rejects_invalid_version() {
    assert!(parse_package_ref("numpy==abc").is_err());
}

#[test]
fn test_parse_package_ref_rejects_unclosed_bracket() {
    assert!(parse_package_ref("numpy[all").is_err());
}

#[test]
fn test_extract_extras_with_version_spec() {
    let (name, extras) = pubgrub::extract_extras("geopandas[all]==5.0.0");
    assert_eq!(name, "geopandas");
    assert_eq!(extras, HashSet::from(["all".to_string()]));
}
```

- [ ] **Step 3: 运行 resolver 测试**

Run: `cargo test --test resolver_tests`
Expected: 全部通过。

- [ ] **Step 4: 提交**

```bash
git add tests/resolver_tests.rs
git commit -m "test(resolver): add parse_package_ref tests"
```

---

### Task 9: 更新 `tests/integration_tests.rs` 中 `collect_pkg_extras` 调用

**Files:**
- Modify: `tests/integration_tests.rs:9` 和 `:84`

- [ ] **Step 1: 更新 imports 和调用**

把 import 改为：

```rust
use pip_mirror::resolver::pubgrub::{bare_name, collect_pkg_refs};
```

把调用改为：

```rust
let extras = collect_pkg_refs(&[package_ref.to_string()])
    .remove(&package)
    .map(|parsed| parsed.extras)
    .unwrap_or_default();
```

- [ ] **Step 2: 编译检查**

Run: `cargo test --test integration_tests --no-run`
Expected: 编译通过。

- [ ] **Step 3: 提交**

```bash
git add tests/integration_tests.rs
git commit -m "test(integration): update for collect_pkg_refs rename"
```

---

### Task 10: 更新 `tests/config_tests.rs` 补充配置校验测试

**Files:**
- Modify: `tests/config_tests.rs`

- [ ] **Step 1: 添加合法版本约束测试**

在 `test_config_backward_compatible_strings_only` 之后插入：

```rust
#[test]
fn test_config_accepts_version_spec() {
    let toml = r#"
packages = [
    "numpy==2.5.0",
    "geopandas[all]==5.0.0",
    "numpy>=1.20,<2.0",
]
repository_dir = "./packages"
"#;
    let cfg: Config = toml::from_str(toml).expect("should parse");
    assert!(cfg.validate().is_ok());
}
```

- [ ] **Step 2: 添加非法版本约束测试**

在 `test_config_accepts_version_spec` 之后插入：

```rust
#[test]
fn test_config_rejects_invalid_version_spec() {
    let toml = r#"
packages = ["numpy==abc"]
repository_dir = "./packages"
"#;
    let cfg: Config = toml::from_str(toml).expect("should parse");
    let err = cfg.validate().expect_err("should fail");
    assert!(err.contains("无效") || err.contains("版本"));
}

#[test]
fn test_config_rejects_space_in_version_spec() {
    let toml = r#"
packages = ["numpy == 2.5.0"]
repository_dir = "./packages"
"#;
    let cfg: Config = toml::from_str(toml).expect("should parse");
    let err = cfg.validate().expect_err("should fail");
    assert!(err.contains("空格"));
}

#[test]
fn test_config_rejects_duplicate_version_spec() {
    let toml = r#"
packages = ["numpy>=1.0", "numpy==2.5.0"]
repository_dir = "./packages"
"#;
    let cfg: Config = toml::from_str(toml).expect("should parse");
    let err = cfg.validate().expect_err("should fail");
    assert!(err.contains("重复") || err.contains("duplicate"));
}
```

- [ ] **Step 3: 运行配置测试**

Run: `cargo test --test config_tests`
Expected: 全部通过。

- [ ] **Step 4: 提交**

```bash
git add tests/config_tests.rs
git commit -m "test(config): add version spec validation tests"
```

---

### Task 11: 更新 `.github/workflows/e2e.yml` 加入真实版本约束案例

**Files:**
- Modify: `.github/workflows/e2e.yml`

- [ ] **Step 1: 在 `wan-inc.toml` 中加入版本约束包**

把 `wan-inc.toml` 的 `packages` 改为：

```toml
packages = [
    "rapidocr-onnxruntime",
    "pyside6",
    "playwright",
    "openai",
    "requests",
    "gradio",
    "streamlit",
    "markitdown[pptx,docx,xls,xlsx,pdf]",
    "certifi==2024.6.2",
]
```

> 注意：`numpy>=1.26.0,<2.0` 作为范围约束在 e2e 中风险较高（可能与深依赖冲突），因此只加精确版本约束 `certifi==2024.6.2`。范围约束已在单元测试中覆盖。

- [ ] **Step 2: 在客户端第二次下载后添加 `certifi` 安装验证**

在 `"客户端第二次下载 — 装增量包（gradio + streamlit + markitdown）"` step 之后插入一个新 step：

```yaml
      - name: '客户端验证精确版本约束 — certifi==2024.6.2'
        run: |
          set -euo pipefail
          uv venv /tmp/client-certifi
          export VIRTUAL_ENV=/tmp/client-certifi
          uv pip install --index-url "http://${MIRROR_HOST}:${MIRROR_PORT}/simple" "certifi==2024.6.2"
          /tmp/client-certifi/bin/python - <<'PY'
          import certifi
          assert certifi.__version__ == '2024.06.02', certifi.__version__
          print("certifi version pinned OK:", certifi.__version__)
          PY
```

- [ ] **Step 3: 提交**

```bash
git add .github/workflows/e2e.yml
git commit -m "ci(e2e): add pinned certifi version spec case"
```

---

### Task 12: 运行完整测试套件并修复回归

**Files:**
- 可能涉及之前所有文件

- [ ] **Step 1: 运行单元测试和集成测试**

Run: `cargo test`
Expected: 全部通过。如果失败，修复对应文件后重新运行。

- [ ] **Step 2: 运行 clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 通过。如果有警告，修复后重新运行。

- [ ] **Step 3: 运行 cargo fmt**

Run: `cargo fmt --check`
Expected: 通过。如果有差异，运行 `cargo fmt` 后提交。

- [ ] **Step 4: 最终提交（如 fmt/clippy 有修改）**

```bash
git add -A
git commit -m "style: cargo fmt and clippy fixes" || echo "no changes to commit"
```

---

## Self-Review

### 1. Spec coverage

| Spec 要求 | 对应 Task |
|----------|----------|
| 支持全 PEP 440 操作符 | Task 1 + Task 2（`parse_package_ref` 提取约束，`validate_version_spec` 校验） |
| 先约束过滤再 take top-N | Task 3（`collect_top_versions`）+ Task 6（`build_top_only_plan`） |
| 不允许空格 | Task 1（`parse_package_ref` 空格检查） |
| 无匹配版本报错 | Task 3 + Task 4（`NoMatchingVersion`） |
| 配置校验 | Task 7 |
| E2E 真实案例 | Task 11 |
| `extract_extras` 修正 | Task 2 |

### 2. Placeholder scan

- 无 TBD/TODO。
- 每个 step 都包含完整代码块、命令和期望输出。
- 没有 "add appropriate error handling" 这类模糊表述。

### 3. Type consistency

- `ParsedPackageRef` 在 Task 1 定义，Task 2 的 `collect_pkg_refs` 返回它，Task 5 从它提取 `version_spec`，类型一致。
- `PlanParams.version_specs` 类型为 `&HashMap<String, Option<String>>`，在 Task 3、5、6 中一致使用。
- `ResolveError::NoMatchingVersion { package, spec }` 在 Task 4 定义，Task 3 和 Task 6 中构造一致。
- `ConfigError::InvalidVersionSpec { package, raw, reason }` 在 Task 7 定义并一致构造。

### 4. 遗漏补充

Spec 中未明确提到 `no_deps` 路径（`build_top_only_plan`）也需要版本过滤，本计划在 Task 6 中补充，确保 `--no-deps` 或类似模式也支持版本约束。
