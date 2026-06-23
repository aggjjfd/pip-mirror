# 配置中支持包版本约束

## 背景与目标

当前 `pip-mirror` 配置里的 `packages` 条目只支持两种形式：

- 字符串包名：`"numpy"`
- 显式 whl URL：`{ url = "file:///..." }`

用户希望像 PEP 508 一样在字符串包名里直接加版本约束，例如：

```toml
packages = [
    "geopandas[all]==5.0.0",
    "numpy>=1.26.0,<2.0",
]
```

这样可以在顶层包层面做版本锁定，而不是无条件取最新 `top_versions_per_package` 个版本。

## 需求澄清结果

1. **支持全 PEP 440 操作符**：`==`, `>=`, `>`, `<=`, `<`, `!=`, `~=`，以及逗号连接的 AND 组合。
2. **版本约束与 top-N 的配合**：版本约束作为前置过滤器，先过滤再取 `top_versions_per_package`。
3. **不允许空格**：格式为 `"geopandas[all]==5.0.0"`，不允许写成 `"geopandas[all] == 5.0.0"`。
4. **无匹配版本时直接报错**：配置指定的版本约束在 PyPI 上找不到匹配版本时，plan 阶段失败退出。

## 方案选择

选择**方案 A：扩展字符串解析**。

理由：
- 与现有 `"markitdown[pptx,docx]"` extras 语法一脉相承，用户体验一致。
- 配置最简洁，不需要新增 `PackageSpec` 变体。
- 改动集中，主要改解析函数和版本过滤两个地方。

## 语法格式

完整格式：

```text
包名[extras]版本约束
```

各部分均可选，但顺序固定：

| 示例 | 解析结果 |
|------|----------|
| `numpy` | name=numpy, extras=∅, constraint=∅ |
| `markitdown[pptx,docx]` | name=markitdown, extras={pptx,docx}, constraint=∅ |
| `numpy==2.5.0` | name=numpy, extras=∅, constraint=`==2.5.0` |
| `geopandas[all]==5.0.0` | name=geopandas, extras={all}, constraint=`==5.0.0` |
| `numpy>=1.20,<2.0` | name=numpy, extras=∅, constraint=`>=1.20,<2.0` |

解析规则：

1. 检查字符串中是否包含空格，包含则报错。
2. 查找 `[` 位置：
   - 无 `[`：整个字符串是包名。
   - 有 `[`：查找对应的 `]`，中间是 extras，后面是版本约束；无 `]` 则报错。
3. `[` 之前是包名，调用 `normalize_package_name()` 归一化。
4. `]` 之后的字符串（如果存在）作为版本约束，做严格格式校验。

> 注意：包名本身只包含 PEP 503 允许的字符 `[a-zA-Z0-9._-]`，因此第一个不属于该集合的字符就是版本约束的起点。

## 数据结构

### `ParsedPackageRef`

新增在 `src/filters.rs`：

```rust
pub struct ParsedPackageRef {
    pub name: String,                    // 归一化后的包名
    pub extras: HashSet<String>,         // extras 集合
    pub version_spec: Option<String>,    // 原始版本约束字符串
}

pub fn parse_package_ref(raw: &str) -> Result<ParsedPackageRef, String> {
    // 1. 空格检查
    // 2. 提取 [extras] 和后面的版本约束
    // 3. 严格校验版本约束格式
    // 4. 归一化包名
}
```

放在 `src/filters.rs` 的原因：该文件已有 `normalize_package_name()`，且被 config 层和 resolver 层共用，是中立位置，避免层级倒置。

### `PlanParams` 新增字段

```rust
pub struct PlanParams<'a> {
    // ... 现有字段 ...
    pub top_packages: &'a [String],
    pub version_specs: &'a HashMap<String, Option<String>>,  // 新增
}
```

key 是归一化包名，value 是版本约束字符串（`None` 表示无约束）。

## 代码改动

### 1. `src/filters.rs`

- 新增 `ParsedPackageRef` 和 `parse_package_ref()`。
- 解析逻辑：
  - 空格检查：含空格直接返回 `Err("版本约束中不允许空格")`。
  - extras 提取：使用 `split_once('[')` 和 `split_once(']')`，正确处理 `numpy[all]==5.0.0` 这种 `]` 后面还带版本约束的场景（现有 `extract_extras` 用 `strip_suffix(']')` 会在这里出错，需要同步修正）。
  - 版本约束校验：对每个逗号分隔的部分，严格解析 `(operator, Version)`。无效版本号（如 `numpy==abc`）必须报错，不能静默跳过。
  - 包名归一化：调用 `normalize_package_name()`。

### 2. `src/resolver/pubgrub.rs`

- 修正 `extract_extras()`：使用 `split_once(']')` 替代 `strip_suffix(']')`，正确处理 `]` 后的版本约束。
- `collect_pkg_extras()` → 改名为 `collect_pkg_refs()`，返回 `HashMap<String, ParsedPackageRef>`。
- `bare_name()` 保持不变，仅去掉 extras。

### 3. `src/resolver/plan/mod.rs`

- `PlanParams` 新增 `version_specs: &HashMap<String, Option<String>>`。
- `build_dependency_plan` / `build_dependency_plan_inner` 新增 `version_specs` 参数。
- `collect_top_versions()` 改动：
  1. 获取 `all_versions`。
  2. 查 `version_specs`，如有约束则用 `spec_to_range()` 转成 `Range<Version>` 过滤。
  3. 过滤后为空 → 返回 `ResolveError::NoMatchingVersion { package, spec }`。
  4. 调用 `select_top_versions()`（过滤后的列表）。

### 4. `src/sync/phases/plan.rs`

- 在 `PlanPhase::run` 中，调用 `collect_pkg_refs()` 解析 `name_pkgs`。
- 从解析结果中提取：
  - extras map → 传入 `build_solve_jobs`
  - version_specs map → 传入 `PlanParams`

### 5. `src/config/validator.rs`

- `ConfigError` 新增变体：
  ```rust
  InvalidVersionSpec { package: String, raw: String, reason: String },
  DuplicateVersionSpec { package: String },
  ```
- `ConfigValidator::validate()` 中新增 `validate_package_refs()` 步骤：
  - 对每个 `PackageSpec::Name` 调用 `parse_package_ref()` 校验格式。
  - 检查同名包重复时，只要其中任意一个条目带有版本约束，就报 `DuplicateVersionSpec`；两个都无约束的同名条目允许重复（等价于一个条目）。

### 6. `src/resolver/error.rs`

- `ResolveError` 新增：
  ```rust
  NoMatchingVersion { package: String, spec: String },
  ```

### 7. `.github/workflows/e2e.yml`

在 `wan-inc.toml` 的 packages 列表中加入版本约束的真实案例：

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
    "numpy>=1.26.0,<2.0",
]
```

并在客户端验证中增加：

```bash
uv pip install --index-url "http://${MIRROR_HOST}:${MIRROR_PORT}/simple" "certifi==2024.6.2"
/tmp/client2/bin/python -c "import certifi; assert certifi.__version__ == '2024.06.02', certifi.__version__"
```

## 数据流

```text
config.packages
    │
    ▼
split_package_specs() ──► name_pkgs: Vec<String>
    │
    ▼
collect_pkg_refs() ──► HashMap<String, ParsedPackageRef>
    │
    ├──► extras map ───────► build_solve_jobs()
    │
    └──► version_specs map ─► PlanParams.version_specs
                              │
                              ▼
                       collect_top_versions()
                              │
                              ├── get_all_versions(package)
                              ├── if version_spec exists:
                              │      filter versions by spec_to_range(spec)
                              ├── if filtered empty: NoMatchingVersion
                              └── select_top_versions(filtered)
```

## 错误处理

| 错误场景 | 错误类型 | 阶段 |
|---------|---------|------|
| 版本约束格式错误（如 `numpy==abc`） | `ConfigError::InvalidVersionSpec` | 配置校验 |
| 字符串中包含空格 | `ConfigError::InvalidVersionSpec` | 配置校验 |
| 同名包重复指定版本约束 | `ConfigError::DuplicateVersionSpec` | 配置校验 |
| 版本约束匹配不到 PyPI 版本 | `ResolveError::NoMatchingVersion` | plan 阶段 |

## 测试

### 单元测试（`src/filters.rs`）

- 纯包名解析：`numpy`
- 仅 extras：`markitdown[pptx,docx]`
- 仅版本约束：`numpy==2.5.0`
- extras + 版本约束：`geopandas[all]==5.0.0`
- 多操作符：`numpy>=1.20,<2.0`
- 空格报错：`numpy == 2.5.0`
- 无效版本号报错：`numpy==abc`
- 无闭合括号报错：`numpy[all`
- 空 extras：`numpy[]==1.0` → extras=∅

### 回归测试（`src/resolver/pubgrub.rs`）

- `extract_extras("geopandas[all]==5.0.0")` → `("geopandas", {"all"})`，不再把 `]==5.0.0` 吞进 extras。

### 解析过滤测试（`tests/resolver_tests.rs` 或 `tests/version_spec_tests.rs`）

- 无约束 → 行为不变，取 top-N
- `==2.5.0` → 只返回 2.5.0
- `>=1.20,<2.0` → 范围内再取 top-N
- 无匹配版本 → `ResolveError::NoMatchingVersion`

### 配置校验测试（`tests/config_tests.rs`）

- 合法约束通过：`numpy==2.5.0`，`numpy>=1.20,<2.0`
- 无效版本号报错 → `InvalidVersionSpec`
- 空格报错 → `InvalidVersionSpec`
- 同名重复约束报错 → `DuplicateVersionSpec`
- 同名一个有约束一个无约束也报错 → `DuplicateVersionSpec`

### E2E 测试（`.github/workflows/e2e.yml`）

- 在 `wan-inc.toml` 加入 `certifi==2024.6.2` 和 `numpy>=1.26.0,<2.0`。
- 客户端验证 `certifi==2024.6.2` 能成功安装且版本正确。
- 验证 numpy 版本 `<2.0`。

## 兼容性

- 不指定版本约束的包行为完全不变。
- `PackageSpec::Url` 不变，URL wheel 不能附加版本约束（因为 URL whl 的版本已在文件名中确定）。
- 所有现有配置无需修改即可继续工作。

## 风险与注意事项

1. **`spec_to_range()` 对无效版本号静默跳过**：这是依赖解析阶段已有的宽松行为。本功能中，用户显式版本约束必须先经过 `parse_package_ref()` 的严格校验，因此不受影响。
2. **`extract_extras()` 必须修正**：原实现用 `strip_suffix(']')` 处理不了 `numpy[all]==5.0.0`，需要改为 `split_once(']')`。
3. **同名包重复约束**：通过配置校验阶段报错，避免运行时歧义。
4. **E2E 包选择需谨慎**：`numpy>=1.26.0,<2.0` 这种约束如果与 markitdown/gradio/streamlit 的深层依赖冲突，会导致 PubGrub 报 `NoSolution`。实现时应先验证依赖关系，必要时换成约束更安全的包（如 `requests>=2.25.0,<3.0`）。
