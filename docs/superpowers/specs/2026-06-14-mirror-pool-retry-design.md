# PyPI 镜像池 + 单次重试设计

## 背景

`pip-mirror` 目前只支持单个 `pypi_url`，且所有 HTTP 请求（元数据、wheel、sdist）都没有重试机制。同步 134 个包时，单次网络抖动就可能让整次 `sync-full` 失败。本设计引入镜像池和每个镜像一次重试，提升同步稳定性。

## 目标

- 支持配置多个 PyPI 镜像地址，顺序优先、失败 fallback。
- 每个镜像对每个请求提供 2 次尝试机会（原始 1 次 + 1 次重试）。
- 仅对网络层错误和 5xx 响应重试；4xx 直接切换镜像。
- 通过 `reqwest-middleware` 实现，避免侵入每个请求点写重复重试代码。
- 向后兼容现有 `pypi_url` 单字符串配置。

## 非目标

- 不支持负载均衡轮询/随机；本次只做顺序主备。
- 不引入指数退避；重试立即发起（或固定短延时）。
- 显式 URL wheel（用户直接指定 URL）不参与镜像 fallback。
- `python_builds` 的 GitHub 下载不属于 PyPI 镜像池，保持现状。

## 设计

### 配置

`src/config.rs`：

```rust
pub struct Config {
    // ...
    #[serde(default = "default_pypi_url")]
    pub pypi_url: String,

    #[serde(default)]
    pub pypi_urls: Vec<String>,
    // ...
}
```

- 合并有效镜像：`pypi_url`（非空时） + `pypi_urls`，去重，保持顺序。
- 默认 `pypi_url = "https://pypi.org"`。
- `src/main.rs` 的 `INIT_TEMPLATE` 示例同步增加 `pypi_urls = ["..."]` 说明。

### 镜像重试中间件

新增 `src/downloader/client.rs`：

```rust
pub struct MirrorRetryMiddleware {
    mirrors: Vec<reqwest::Url>,
}

#[async_trait::async_trait]
impl reqwest_middleware::Middleware for MirrorRetryMiddleware {
    async fn handle(
        &self,
        req: reqwest::Request,
        extensions: &mut reqwest_middleware::Extensions,
        next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        // 保存原始 path + query
        // 对每个 mirror origin：
        //   构造新 URL
        //   for attempt in 0..2:
        //     发送
        //     成功 -> 返回
        //     5xx/timeout/connection -> 继续下一次 attempt
        //     4xx/其它 -> break 到下一个 mirror
        // 返回最后一次错误
    }
}
```

重试判定：

- `err.is_timeout()`
- `err.is_connect()`
- `err.status().map_or(false, |s| s.is_server_error())`

日志：每次切换镜像或重试时输出 `tracing::warn!`。

### Client 构建

`src/sync/mod.rs` 的 `build_sync_client()`：

```rust
pub fn build_sync_client(
    mirrors: Vec<String>,
) -> Result<reqwest_middleware::ClientWithMiddleware, reqwest::Error> {
    let inner = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;

    let origins = mirrors
        .into_iter()
        .filter(|s| !s.is_empty())
        .map(|s| reqwest::Url::parse(&s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| {
            reqwest::Error::from(e) // 或自定义错误
        })?;

    Ok(reqwest_middleware::ClientBuilder::new(inner)
        .with(MirrorRetryMiddleware::new(origins))
        .build())
}
```

### 代码改造

- `MetadataCache` 移除 `pypi_url: String`，只保留 `client: ClientWithMiddleware`。
- 元数据请求 URL 直接请求绝对 URL（如 `https://pypi.org/pypi/pytz/json`），中间件负责把 origin 替换成各镜像 origin。
- 下载请求同理：`FileInfo.url` 是 `files.pythonhosted.org` 的绝对 URL，中间件替换 origin 到各镜像。
- 所有 `&reqwest::Client` 改为 `&ClientWithMiddleware`：
  - `src/downloader.rs`
  - `src/resolver/build_requires.rs`
  - `src/sync/url_wheel_download.rs`
  - `src/sync/mod.rs`
  - `src/sync/plan.rs`
  - `src/resolver/plan.rs`
  - `src/resolver/metadata.rs`
- `.send().await` 的返回错误改为 `reqwest_middleware::Error`；上层提取 HTTP status 或 source 时通过 `err.as_reqwest_error()` 转换。

### 显式 URL wheel

`src/sync/url_wheel_download.rs` 中的显式 URL 请求仍使用带中间件的 client，但中间件会尝试替换 origin。由于显式 URL 可能不是 PyPI 结构，这可能导致向错误镜像发送请求。

处理方式：在中间件中识别非 PyPI origin（非 `pypi.org` 且非配置中的 mirror origin）的请求，直接透传，不做镜像 fallback。更简单的方式：为显式 URL 请求单独构建一个无中间件的 `reqwest::Client`。

本次采用后者：显式 URL wheel 单独使用裸 `reqwest::Client`（300s timeout），不参与镜像池。

## 测试

1. **配置解析测试**：
   - 仅 `pypi_url` -> 合并后 1 个镜像。
   - `pypi_url` + `pypi_urls` -> 顺序正确且去重。
   - 空配置 -> 默认镜像。

2. **中间件单元测试**（使用 `wiremock` 或本地 `tokio::net::TcpListener`）：
   - 首个镜像 500 后 200 -> 成功。
   - 首个镜像连续两次 500 -> 切换到第二个镜像。
   - 首个镜像 404 -> 切换到第二个镜像。
   - 所有镜像 500 -> 返回 500 错误。

3. **集成回归**：
   - 运行 `cargo test --test resolver_regression_tests` 等，确保签名改动不影响现有测试。

## 验收标准

- `cargo clippy --all-targets`、`cargo test`、`check-complexity.py` 全部通过。
- 配置支持 `pypi_url` 单字符串和 `pypi_urls` 数组，向后兼容。
- 第一个镜像 500 或 timeout 时，请求自动在同镜像重试一次；仍失败则切换下一个镜像。
- 4xx 不触发同镜像重试，直接尝试下一个镜像。
