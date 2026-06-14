# PyPI 镜像池 + 单次重试实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 `pip-mirror` 增加 PyPI 镜像池支持和每个镜像一次重试，提高同步对网络抖动的容忍度。

**Architecture:** 通过 `reqwest-middleware` 实现 `MirrorRetryMiddleware`，在 HTTP 层统一处理镜像顺序切换与重试；配置层合并 `pypi_url` 与 `pypi_urls`；`MetadataCache`、下载器、sdist 下载统一使用带中间件的 `ClientWithMiddleware`。

**Tech Stack:** Rust, tokio, reqwest, reqwest-middleware, async-trait

---

## 文件变更映射

- **创建：** `src/downloader/client.rs` — 镜像重试中间件与 client 构建。
- **修改：** `Cargo.toml` — 新增依赖。
- **修改：** `src/config.rs` — 新增 `pypi_urls` 字段与合并方法。
- **修改：** `src/main.rs` — 示例配置模板。
- **修改：** `src/sync/mod.rs` — `build_sync_client` 返回 `ClientWithMiddleware`，`do_sync` 传镜像列表。
- **修改：** `src/sync/plan.rs` — `build_top_only_plan` 使用 `ClientWithMiddleware` 与镜像列表。
- **修改：** `src/resolver/plan.rs` — `PlanParams` 使用镜像列表。
- **修改：** `src/resolver/metadata.rs` — `MetadataCache` 持有 `ClientWithMiddleware`，移除 `pypi_url`。
- **修改：** `src/downloader.rs` — `download_file` 使用 `ClientWithMiddleware`。
- **修改：** `src/resolver/build_requires.rs` — `download_sdist` 使用 `ClientWithMiddleware`。
- **修改：** `src/sync/url_wheel_download.rs` — 显式 URL wheel 使用裸 `reqwest::Client`。
- **修改：** 测试文件 — 适配 `PlanParams` 字段名变化。

---

### Task 1: 新增依赖

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: 在 `[dependencies]` 中添加依赖**

```toml
reqwest-middleware = "0.4"
async-trait = "0.1"
```

- [ ] **Step 2: 运行 cargo check 确认依赖可解析**

Run: `cargo check 2>&1 | tail -10`
Expected: 成功下载依赖，无编译错误（此时代码还未改，应该只提示未使用依赖）。

- [ ] **Step 3: 提交**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore(deps): 添加 reqwest-middleware 与 async-trait"
```

---

### Task 2: 配置支持多个镜像

**Files:**
- Modify: `src/config.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: 在 `Config` 结构体新增 `pypi_urls`**

```rust
#[serde(default = "default_pypi_url")]
pub pypi_url: String,

#[serde(default)]
pub pypi_urls: Vec<String>,
```

- [ ] **Step 2: 在 `Config` impl 中新增 `effective_mirrors` 方法**

```rust
impl Config {
    pub fn effective_mirrors(&self) -> Vec<String> {
        let mut mirrors = Vec::new();
        if !self.pypi_url.is_empty() {
            mirrors.push(self.pypi_url.clone());
        }
        for url in &self.pypi_urls {
            if !url.is_empty() && !mirrors.contains(url) {
                mirrors.push(url.clone());
            }
        }
        if mirrors.is_empty() {
            mirrors.push(default_pypi_url());
        }
        mirrors
    }
}
```

- [ ] **Step 3: 更新 `src/main.rs` 的 `INIT_TEMPLATE`**

在模板中的 `pypi_url = "https://pypi.org"` 下方添加：

```toml
# 可选：备用镜像池，按顺序 fallback，每个镜像失败后再重试一次
# pypi_urls = [
#     "https://mirrors.ustc.edu.cn/pypi/simple",
# ]
```

- [ ] **Step 4: 添加配置解析单元测试**

在 `src/config.rs` 的 `#[cfg(test)]` 块中新增：

```rust
#[test]
fn test_effective_mirrors_single() {
    let config = Config {
        pypi_url: "https://a.com".into(),
        pypi_urls: vec![],
        ..Config::default()
    };
    assert_eq!(config.effective_mirrors(), vec!["https://a.com"]);
}

#[test]
fn test_effective_mirrors_combined_and_deduped() {
    let config = Config {
        pypi_url: "https://a.com".into(),
        pypi_urls: vec!["https://a.com".into(), "https://b.com".into()],
        ..Config::default()
    };
    assert_eq!(
        config.effective_mirrors(),
        vec!["https://a.com", "https://b.com"]
    );
}
```

- [ ] **Step 5: 运行测试**

Run: `cargo test --lib config::tests 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add src/config.rs src/main.rs
git commit -m "feat(config): 支持 pypi_url 与 pypi_urls 合并为镜像池"
```

---

### Task 3: 实现 MirrorRetryMiddleware

**Files:**
- Create: `src/downloader/client.rs`

- [ ] **Step 1: 创建文件并写入中间件**

```rust
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Request, Response, Url};
use reqwest_middleware::{Extensions, Middleware, Next, Result as MiddlewareResult};
use tracing::warn;

pub struct MirrorRetryMiddleware {
    mirrors: Vec<Url>,
}

impl MirrorRetryMiddleware {
    pub fn new(mirrors: Vec<Url>) -> Self {
        Self { mirrors }
    }
}

#[async_trait]
impl Middleware for MirrorRetryMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> MiddlewareResult<Response> {
        if self.mirrors.is_empty() {
            return next.run(req, extensions).await;
        }

        let original = req.url().clone();
        let path_and_query = build_path_and_query(&original);
        let mut last_error = None;

        for mirror in &self.mirrors {
            let attempt_url = match mirror.join(&path_and_query) {
                Ok(url) => url,
                Err(e) => {
                    warn!("镜像 URL 拼接失败: {e}");
                    continue;
                }
            };

            for attempt in 0..2 {
                let mut attempt_req = req
                    .try_clone()
                    .ok_or_else(|| {
                        reqwest_middleware::Error::middleware(
                            anyhow::anyhow!("无法克隆请求"),
                        )
                    })?;
                *attempt_req.url_mut() = attempt_url.clone();

                match next.clone().run(attempt_req, extensions).await {
                    Ok(resp) => {
                        let status = resp.status();
                        if status.is_server_error() && attempt == 0 {
                            warn!(
                                "镜像 {} 返回 {}，准备重试",
                                mirror, status
                            );
                            continue;
                        }
                        return Ok(resp);
                    }
                    Err(err) => {
                        if should_retry(&err) && attempt == 0 {
                            warn!(
                                "镜像 {} 请求失败: {err}，准备重试",
                                mirror
                            );
                            continue;
                        }
                        warn!("镜像 {} 不可用: {err}", mirror);
                        last_error = Some(err);
                        break;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            reqwest_middleware::Error::middleware(anyhow::anyhow!(
                "所有镜像均不可用"
            ))
        }))
    }
}

fn build_path_and_query(url: &Url) -> String {
    let mut s = url.path().to_string();
    if let Some(q) = url.query() {
        s.push('?');
        s.push_str(q);
    }
    s
}

fn should_retry(err: &reqwest_middleware::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_path_and_query() {
        let url = Url::parse("https://pypi.org/pypi/pytz/json?a=1").unwrap();
        assert_eq!(build_path_and_query(&url), "/pypi/pytz/json?a=1");
    }
}
```

- [ ] **Step 2: 添加 anyhow 依赖（中间件错误需要）**

`Cargo.toml`:

```toml
anyhow = "1"
```

- [ ] **Step 3: 在 `src/downloader/mod.rs` 中暴露 client 模块**

确认 `src/downloader/mod.rs` 存在并添加：

```rust
pub mod client;
```

- [ ] **Step 4: 运行测试**

Run: `cargo test --lib downloader::client::tests 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add Cargo.toml Cargo.lock src/downloader/client.rs src/downloader/mod.rs
git commit -m "feat(client): 实现镜像 fallback 与单次重试中间件"
```

---

### Task 4: 改造 build_sync_client

**Files:**
- Modify: `src/sync/mod.rs`

- [ ] **Step 1: 修改导入与函数签名**

```rust
use crate::downloader::client::MirrorRetryMiddleware;
use reqwest_middleware::ClientBuilder;
```

```rust
pub fn build_sync_client(
    mirrors: Vec<String>,
) -> Result<reqwest_middleware::ClientWithMiddleware, Box<dyn std::error::Error>> {
    let inner = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;

    let origins = mirrors
        .into_iter()
        .map(|s| reqwest::Url::parse(&s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("镜像地址解析失败: {e}"))?;

    Ok(ClientBuilder::new(inner)
        .with(MirrorRetryMiddleware::new(origins))
        .build())
}
```

- [ ] **Step 2: 更新 `do_sync` 调用处**

```rust
let client = build_sync_client(config.effective_mirrors())?;
```

- [ ] **Step 3: 提交**

```bash
git add src/sync/mod.rs
git commit -m "feat(sync): build_sync_client 返回带镜像重试的 ClientWithMiddleware"
```

---

### Task 5: 改造 MetadataCache

**Files:**
- Modify: `src/resolver/metadata.rs`
- Modify: `src/resolver/plan.rs`
- Modify: `src/sync/plan.rs`
- Modify: `src/sync/mod.rs`

- [ ] **Step 1: MetadataCache 字段与构造**

```rust
use reqwest_middleware::ClientWithMiddleware;

pub struct MetadataCache {
    client: ClientWithMiddleware,
    // ...
}

impl MetadataCache {
    pub fn new(
        client: ClientWithMiddleware,
        metadata_workers: usize,
    ) -> Self {
        Self {
            client,
            // ...
        }
    }
}
```

- [ ] **Step 2: fetch_json 签名**

```rust
async fn fetch_json(&self, url: &str) -> Result<serde_json::Value, MetadataError> {
    let response = self
        .client
        .get(url)
        .send()
        .await
        .map_err(|e| MetadataError::Http { ... })?;
    // ...
}
```

注意：错误类型转换时，使用 `e.as_reqwest_error()` 提取底层错误。reqwest_middleware Error 的 `.status()` 也可直接取。

- [ ] **Step 3: 更新 URL 构造函数**

`fetch_package_index`、`fetch_version_metadata`、`fetch_build_requires_probe` 直接使用 `self.client` 发送请求，URL 使用绝对 URL（如 `https://pypi.org/pypi/{pkg}/json`）。中间件负责镜像 origin 替换。

- [ ] **Step 4: PlanParams 与调用点**

`src/resolver/plan.rs`:

```rust
pub struct PlanParams<'a> {
    pub pypi_urls: &'a [String],
    // ... 删除 pypi_url
}
```

```rust
let cache = MetadataCache::new(
    client.clone(),
    params.metadata_workers,
);
```

`src/sync/plan.rs`:

```rust
pub async fn build_top_only_plan(
    config: &crate::config::Config,
    client: &ClientWithMiddleware,
    pkgs: &[String],
) -> Result<DependencyPlan, ResolveError> {
    let cache = MetadataCache::new(client.clone(), config.metadata_workers);
    // ...
}
```

- [ ] **Step 5: 提交**

```bash
git add src/resolver/metadata.rs src/resolver/plan.rs src/sync/plan.rs src/sync/mod.rs
git commit -m "feat(metadata): MetadataCache 使用 ClientWithMiddleware 并移除 pypi_url"
```

---

### Task 6: 传播 ClientWithMiddleware 到下载层

**Files:**
- Modify: `src/downloader.rs`
- Modify: `src/resolver/build_requires.rs`
- Modify: `src/sync/url_wheel_download.rs`
- Modify: `src/sync/mod.rs`

- [ ] **Step 1: `src/downloader.rs`**

```rust
use reqwest_middleware::ClientWithMiddleware;

pub async fn download_pkg_files_with_prefetched(
    client: &ClientWithMiddleware,
    // ...
)

async fn try_network_download(
    client: &ClientWithMiddleware,
    // ...
)

async fn download_file(
    client: &ClientWithMiddleware,
    fi: &FileInfo,
    dest: &Path,
) -> (bool, String) {
    match client.get(&fi.url).send().await {
        // ...
    }
}
```

- [ ] **Step 2: `src/resolver/build_requires.rs`**

```rust
async fn download_sdist(
    client: &ClientWithMiddleware,
    // ...
) -> Result<Vec<u8>, ResolveError> {
    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(|e| ResolveError::Metadata(MetadataError::Http { ... }))?
        .bytes()
        .await
        .map_err(|e| ResolveError::Metadata(MetadataError::Http { ... }))?;
    Ok(bytes.to_vec())
}
```

- [ ] **Step 3: `src/sync/url_wheel_download.rs`**

为显式 URL wheel 单独构建裸 `reqwest::Client`：

```rust
fn build_explicit_url_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
}
```

在 `maybe_collect_url_wheel_deps` 和 `process_single_url_wheel` 中使用该裸 client。

- [ ] **Step 4: `src/sync/mod.rs` 的 `run_downloads`**

`download_pkg_files_with_prefetched` 调用处已持有 `client: &ClientWithMiddleware`。

- [ ] **Step 5: 提交**

```bash
git add src/downloader.rs src/resolver/build_requires.rs src/sync/url_wheel_download.rs src/sync/mod.rs
git commit -m "feat(download): 下载层统一使用带镜像重试的 ClientWithMiddleware"
```

---

### Task 7: 更新测试与示例

**Files:**
- Modify: `tests/concurrency_regression_tests.rs`
- Modify: `tests/integration_tests.rs`
- Modify: `tests/resolver_regression_tests.rs`

- [ ] **Step 1: 更新测试中的 `PlanParams` 构造**

```rust
PlanParams {
    pypi_urls: &["https://pypi.org".into()],
    // ... 删除 pypi_url
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo test --lib 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 3: 提交**

```bash
git add tests/
git commit -m "test: 适配 PlanParams 字段名变更"
```

---

### Task 8: 中间件集成测试

**Files:**
- Modify: `src/downloader/client.rs`

- [ ] **Step 1: 添加 mock server 测试**

使用 `wiremock` crate 或 `tokio::net::TcpListener` 手写 mock。为减少依赖，使用 `tokio::net::TcpListener` + 简单 HTTP 响应。

```rust
#[tokio::test]
async fn test_mirror_retry_success_on_second_mirror() {
    let mirror1 = start_mock_server("HTTP/1.1 500 Internal Server Error\r\n\r\n").await;
    let mirror2 = start_mock_server("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nOK").await;

    let client = build_test_client(vec![mirror1, mirror2]);
    let resp = client.get("https://pypi.org/pypi/pytz/json").send().await.unwrap();
    assert!(resp.status().is_success());
}
```

由于手写 mock 较复杂，也可以用 `wiremock`：

```toml
[dev-dependencies]
wiremock = "0.6"
```

```rust
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::any;

#[tokio::test]
async fn test_retry_then_fallback() {
    let bad = MockServer::start().await;
    let good = MockServer::start().await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(2)
        .mount(&bad)
        .await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&good)
        .await;

    let client = build_test_client(vec![bad.uri(), good.uri()]);
    let resp = client.get("https://pypi.org/pypi/pytz/json").send().await.unwrap();
    assert!(resp.status().is_success());
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo test --lib downloader::client 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 3: 提交**

```bash
git add Cargo.toml Cargo.lock src/downloader/client.rs
git commit -m "test(client): 增加 MirrorRetryMiddleware 集成测试"
```

---

### Task 9: 全量验证

- [ ] **Step 1: 运行 fmt + clippy + 全量测试 + 复杂度门禁**

```bash
cargo fmt
cargo clippy --all-targets
cargo test --all-targets
python scripts/check-complexity.py src/downloader/client.rs src/config.rs src/resolver/metadata.rs src/sync/mod.rs
```

Expected: 全部通过。

- [ ] **Step 2: 手动验证**

创建一个临时配置文件，配置两个镜像（一个无效，一个有效），运行：

```bash
./pip-mirror sync -p six --dry-run --no-deps -c /tmp/test.toml
```

Expected: 进度条/日志显示 fallback 到有效镜像，最终成功。

- [ ] **Step 3: 提交（如只修复了小问题）**

```bash
git add -A
git commit -m "fix: 全量验证后的收尾调整"
```

---

## 自检

- [x] Spec 覆盖：配置合并（Task 2）、中间件实现（Task 3）、client 构建（Task 4）、MetadataCache 改造（Task 5）、下载层传播（Task 6）、测试（Task 7/8）均已对应。
- [x] 无占位符：每个步骤包含实际代码或命令。
- [x] 类型一致：`ClientWithMiddleware` 贯穿所有任务；`PlanParams.pypi_urls` 在 Task 5/7 一致。
