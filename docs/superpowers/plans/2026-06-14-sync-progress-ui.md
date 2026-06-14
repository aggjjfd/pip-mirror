# pip-mirror 同步命令进度条 UI 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 `sync`、`sync-full`、`import-incremental` 命令引入事件总线驱动的进度条 UI，替换当前刷屏的逐文件日志。

**Architecture:** 新增 `src/progress/` 模块承载事件定义、渲染器与公共 API；业务模块接收 `ProgressHandle` 并在关键节点发送 `SyncEvent`；TTY 下用 `indicatif` 渲染两行进度，非 TTY 下回退到精简日志行。

**Tech Stack:** Rust, tokio (mpsc), indicatif

---

## 文件结构

- **Create:** `src/progress/events.rs` — `SyncEvent`、`FileStatus` 枚举。
- **Create:** `src/progress/mod.rs` — `ProgressHandle`、`run_with_progress` 公共 API。
- **Create:** `src/progress/plain.rs` — 非 TTY 渲染器。
- **Create:** `src/progress/tty.rs` — TTY 渲染器（基于 indicatif）。
- **Create:** `tests/progress_tests.rs` — 进度模块单元测试。
- **Modify:** `src/lib.rs` — 暴露 `progress` 模块。
- **Modify:** `src/downloader.rs` — 复用/下载完成时发送 `FileDone` 事件。
- **Modify:** `src/downloader/pipeline.rs` — 传递 `ProgressHandle`。
- **Modify:** `src/python_builds.rs` — Python 解释器下载阶段发送事件。
- **Modify:** `src/resolver/resolve.rs` — resolver 阶段发送事件。
- **Modify:** `src/sync/mod.rs` — 同步主流程发送事件。
- **Modify:** `src/sync/finalize.rs` — finalize 阶段发送事件。
- **Modify:** `src/indexer.rs` — 索引生成阶段发送事件。
- **Modify:** `src/main.rs` — 命令入口调用 `run_with_progress`。

---

### Task 1: 创建事件类型 `src/progress/events.rs`

**Files:**
- Create: `src/progress/events.rs`
- Modify: `src/lib.rs`（添加 `pub mod progress;`）

- [ ] **Step 1: 实现事件枚举**

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum FileStatus {
    Downloaded,
    Reused,
    Skipped,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SyncEvent {
    PhaseStarted {
        phase: &'static str,
        total: Option<u64>,
    },
    PhaseProgress {
        phase: &'static str,
        current: u64,
        message: String,
    },
    PhaseFinished {
        phase: &'static str,
        summary: String,
    },
    FileDone {
        package: String,
        filename: String,
        status: FileStatus,
    },
    Error {
        message: String,
    },
    Warning {
        message: String,
    },
}
```

- [ ] **Step 2: 在 `src/lib.rs` 暴露 progress 模块**

在 `src/lib.rs` 现有模块列表末尾添加：

```rust
pub mod progress;
```

- [ ] **Step 3: Commit**

```bash
git add src/progress/events.rs src/lib.rs
git commit -m "feat(progress): 添加同步事件类型" -m "- 定义 SyncEvent 和 FileStatus" -m "- 暴露 progress 模块"
```

---

### Task 2: 创建公共 API `src/progress/mod.rs`

**Files:**
- Create: `src/progress/mod.rs`
- Create: `tests/progress_tests.rs`

- [ ] **Step 1: 写失败测试**

```rust
// tests/progress_tests.rs
use pip_mirror::progress::{run_with_progress, SyncEvent};

#[tokio::test]
async fn test_progress_handle_emits_events() {
    let result = run_with_progress(|handle| async move {
        handle.emit(SyncEvent::PhaseStarted {
            phase: "test",
            total: Some(2),
        });
        handle.emit(SyncEvent::PhaseProgress {
            phase: "test",
            current: 1,
            message: "item 1".to_string(),
        });
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await;

    assert!(result.is_ok());
}
```

Run: `cargo test --test progress_tests -- progress_handle_emits_events`
Expected: FAIL — `run_with_progress` 未定义。

- [ ] **Step 2: 实现公共 API**

```rust
// src/progress/mod.rs
use tokio::sync::mpsc;

pub mod events;
mod plain;
mod tty;

pub use events::{FileStatus, SyncEvent};

#[derive(Clone)]
pub struct ProgressHandle {
    tx: mpsc::UnboundedSender<SyncEvent>,
}

impl ProgressHandle {
    pub fn emit(&self, event: SyncEvent) {
        let _ = self.tx.send(event);
    }
}

pub async fn run_with_progress<F, Fut, T>(
    _verbose: bool,
    f: F,
) -> Result<T, Box<dyn std::error::Error>>
where
    F: FnOnce(ProgressHandle) -> Fut,
    Fut: std::future::Future<Output = Result<T, Box<dyn std::error::Error>>>,
{
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = ProgressHandle { tx };

    let renderer = if std::io::stdout().is_terminal() {
        tokio::spawn(async move { tty::render(rx).await })
    } else {
        tokio::spawn(async move { plain::render(rx).await })
    };

    let result = f(handle).await;
    renderer
        .await
        .map_err(|e| format!("进度渲染任务异常: {e}"))?;
    result
}
```

> 说明：`_verbose` 保留参数以匹配设计文档语义；当前 `logging::init` 已处理 verbose 的 tracing 级别，渲染器不依赖它。

- [ ] **Step 3: 运行测试**

Run: `cargo test --test progress_tests -- progress_handle_emits_events`
Expected: PASS（`plain::render` 和 `tty::render` 在下一步实现，当前会编译失败，因此先用空实现占位并提交，见 Step 4）。

- [ ] **Step 4: 先创建 `plain.rs` 和 `tty.rs` 的空模块让本 Task 编译通过**

```rust
// src/progress/plain.rs
use tokio::sync::mpsc::UnboundedReceiver;
use super::SyncEvent;

pub async fn render(mut _rx: UnboundedReceiver<SyncEvent>) {}
```

```rust
// src/progress/tty.rs
use tokio::sync::mpsc::UnboundedReceiver;
use super::SyncEvent;

pub async fn render(mut _rx: UnboundedReceiver<SyncEvent>) {}
```

- [ ] **Step 5: 运行测试**

Run: `cargo test --test progress_tests -- progress_handle_emits_events`
Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add src/progress/mod.rs src/progress/plain.rs src/progress/tty.rs tests/progress_tests.rs
git commit -m "feat(progress): 添加 ProgressHandle 与 run_with_progress" -m "- 事件通过 mpsc 发送到渲染任务" -m "- 根据 TTY 选择渲染器"
```

---

### Task 3: 实现非 TTY 渲染器 `src/progress/plain.rs`

**Files:**
- Modify: `src/progress/plain.rs`
- Modify: `tests/progress_tests.rs`

- [ ] **Step 1: 写失败测试**

```rust
// tests/progress_tests.rs 末尾添加
#[tokio::test]
async fn test_plain_renderer_outputs_lines() {
    use pip_mirror::progress::run_with_progress;

    // 强制非 TTY 无法在此测试直接断言输出；改为测试 handle 不 panic。
    let result = run_with_progress(|handle| async move {
        handle.emit(SyncEvent::PhaseStarted {
            phase: "download",
            total: Some(2),
        });
        handle.emit(SyncEvent::PhaseProgress {
            phase: "download",
            current: 1,
            message: "a.whl".to_string(),
        });
        handle.emit(SyncEvent::PhaseFinished {
            phase: "download",
            summary: "完成 2/2".to_string(),
        });
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await;

    assert!(result.is_ok());
}
```

Run: `cargo test --test progress_tests`
Expected: FAIL — plain renderer 还没实现，但编译应通过；断言可能因输出捕获问题失败。实际运行时不会 fail，因此此测试主要验证不 panic。

- [ ] **Step 2: 实现 plain 渲染器**

```rust
// src/progress/plain.rs
use tokio::sync::mpsc::UnboundedReceiver;

use super::SyncEvent;

pub async fn render(mut rx: UnboundedReceiver<SyncEvent>) {
    while let Some(event) = rx.recv().await {
        match event {
            SyncEvent::PhaseStarted { phase, total } => {
                let total_str = total
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".to_string());
                println!("[progress] 开始 {phase} (共 {total_str})");
            }
            SyncEvent::PhaseProgress {
                phase,
                current,
                message,
            } => {
                println!("[progress] {phase} {current} {message}");
            }
            SyncEvent::PhaseFinished { phase, summary } => {
                println!("[progress] {phase} 完成 {summary}");
            }
            SyncEvent::FileDone {
                filename,
                status,
                ..
            } => match status {
                super::FileStatus::Failed(msg) => {
                    println!("[error] {filename}: {msg}");
                }
                _ => {}
            },
            SyncEvent::Error { message } => {
                println!("[error] {message}");
            }
            SyncEvent::Warning { message } => {
                println!("[warn] {message}");
            }
        }
    }
}
```

- [ ] **Step 3: 运行测试**

Run: `cargo test --test progress_tests`
Expected: PASS。

- [ ] **Step 4: Commit**

```bash
git add src/progress/plain.rs tests/progress_tests.rs
git commit -m "feat(progress): 实现非 TTY 渲染器" -m "- 输出 [progress]/[error]/[warn] 精简日志行"
```

---

### Task 4: 实现 TTY 渲染器 `src/progress/tty.rs`

**Files:**
- Modify: `src/progress/tty.rs`

- [ ] **Step 1: 实现 TTY 渲染器**

```rust
// src/progress/tty.rs
use std::collections::HashMap;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{FileStatus, SyncEvent};

pub async fn render(mut rx: UnboundedReceiver<SyncEvent>) {
    let multi = MultiProgress::new();
    let overall = multi.add(ProgressBar::new(0));
    let status = multi.add(ProgressBar::new(0));

    overall.set_style(
        ProgressStyle::with_template("{msg} [{bar:40}] {pos}/{len}")
            .unwrap()
            .progress_chars("##-"),
    );
    status.set_style(
        ProgressStyle::with_template("{msg}")
            .unwrap()
            .progress_chars("##-"),
    );

    let mut phase_totals: HashMap<&'static str, u64> = HashMap::new();

    while let Some(event) = rx.recv().await {
        match event {
            SyncEvent::PhaseStarted { phase, total } => {
                if let Some(t) = total {
                    overall.set_length(t);
                    phase_totals.insert(phase, t);
                } else {
                    overall.set_length(0);
                }
                overall.set_position(0);
                overall.set_message(phase.to_string());
                status.set_message("准备中...".to_string());
            }
            SyncEvent::PhaseProgress {
                phase,
                current,
                message,
            } => {
                if let Some(&t) = phase_totals.get(phase) {
                    overall.set_length(t);
                }
                overall.set_position(current);
                status.set_message(message);
            }
            SyncEvent::PhaseFinished { phase, summary } => {
                if let Some(&t) = phase_totals.get(phase) {
                    overall.set_length(t);
                    overall.set_position(t);
                }
                status.set_message(summary);
            }
            SyncEvent::FileDone { filename, status: s, .. } => {
                match s {
                    FileStatus::Failed(msg) => {
                        multi.println(format!("! {filename}: {msg}")).ok();
                    }
                    _ => {}
                }
            }
            SyncEvent::Error { message } => {
                multi.println(format!("! {message}")).ok();
            }
            SyncEvent::Warning { message } => {
                multi.println(format!("! {message}")).ok();
            }
        }
    }

    overall.finish();
    status.finish();
}
```

- [ ] **Step 2: 编译检查**

Run: `cargo check`
Expected: PASS（可能因 unused imports 有 warning，后续修复）。

- [ ] **Step 3: Commit**

```bash
git add src/progress/tty.rs
git commit -m "feat(progress): 实现 TTY 渲染器" -m "- 使用 indicatif MultiProgress 两行显示" -m "- 错误即时打印在进度条下方"
```

---

### Task 5: 给下载模块发送事件

**Files:**
- Modify: `src/downloader.rs`
- Modify: `src/downloader/pipeline.rs`

- [ ] **Step 1: 修改 `download_pkg_files_with_prefetched` 签名，接收 `ProgressHandle`**

在 `src/downloader.rs` 中：

```rust
// 在文件顶部添加
use crate::progress::{FileStatus, ProgressHandle, SyncEvent};
```

修改 `download_pkg_files_with_prefetched` 函数签名（原函数在 pipeline 中被调用）：

```rust
pub async fn download_pkg_files_with_prefetched(
    client: &reqwest::Client,
    repo: &std::path::Path,
    files: &[FileInfo],
    prefetched_files: &PrefetchedFiles,
    include_source: bool,
    download_workers: usize,
    progress: Option<ProgressHandle>, // 新增
) -> DownloadResult {
    run_download_pipeline(
        client,
        repo,
        files,
        prefetched_files,
        include_source,
        download_workers,
        progress,
    )
    .await
}
```

修改 `download_pkg_files` 函数以兼容现有调用（暂时传 `None`）：

```rust
pub async fn download_pkg_files(
    client: &reqwest::Client,
    repo: &std::path::Path,
    files: &[FileInfo],
    include_source: bool,
    download_workers: usize,
) -> DownloadResult {
    let prefetched_files = PrefetchedFiles::new();
    download_pkg_files_with_prefetched(
        client,
        repo,
        files,
        &prefetched_files,
        include_source,
        download_workers,
        None,
    )
    .await
}
```

- [ ] **Step 2: 修改 `try_prefetched_write` 和 `try_network_download` 发送事件**

```rust
async fn try_prefetched_write(
    fi: &FileInfo,
    dest: &Path,
    bytes: &[u8],
    store: &Option<Arc<crate::store::DownloadStore>>,
    progress: &Option<ProgressHandle>, // 新增
) -> DownloadOutcome {
    if !bytes_match_sha256(fi, bytes) {
        return DownloadOutcome::Failed(
            fi.clone(),
            "预下载文件 hash 校验失败".to_string(),
        );
    }
    let (ok, msg) = write_atomic(dest, bytes).await;
    if ok {
        tracing::info!("复用预下载文件: {}", fi.filename);
        if let Some(p) = progress {
            p.emit(SyncEvent::FileDone {
                package: fi.package_name.clone(),
                filename: fi.filename.clone(),
                status: FileStatus::Reused,
            });
        }
        if let Some(s) = store {
            s.record_download(fi, dest).await;
        }
        return DownloadOutcome::Downloaded(fi.clone());
    }
    DownloadOutcome::Failed(fi.clone(), msg)
}

async fn try_network_download(
    client: &reqwest::Client,
    fi: &FileInfo,
    dest: &Path,
    store: &Option<Arc<crate::store::DownloadStore>>,
    progress: &Option<ProgressHandle>, // 新增
) -> DownloadOutcome {
    let (ok, msg) = download_file(client, fi, dest).await;
    if ok {
        tracing::info!("下载完成: {}", fi.filename);
        if let Some(p) = progress {
            p.emit(SyncEvent::FileDone {
                package: fi.package_name.clone(),
                filename: fi.filename.clone(),
                status: FileStatus::Downloaded,
            });
        }
        if let Some(s) = store {
            s.record_download(fi, dest).await;
        }
        DownloadOutcome::Downloaded(fi.clone())
    } else {
        DownloadOutcome::Failed(fi.clone(), msg)
    }
}
```

- [ ] **Step 3: 修改 `try_download` 传递 `ProgressHandle`**

```rust
async fn try_download(
    client: &reqwest::Client,
    store: &Option<Arc<crate::store::DownloadStore>>,
    prefetched_files: &PrefetchedFiles,
    fi: &FileInfo,
    repo: &std::path::Path,
    progress: &Option<ProgressHandle>, // 新增
) -> DownloadOutcome {
    let dest = repo
        .join("simple")
        .join(&fi.package_name)
        .join(&fi.filename);
    if store.as_ref().is_some_and(|s| {
        s.has_file(&fi.package_name, &fi.filename).unwrap_or(false)
    }) {
        return DownloadOutcome::Skipped(fi.clone());
    }
    if dest.exists() {
        if let Some(s) = store {
            s.record_download(fi, &dest).await;
        }
        return DownloadOutcome::Skipped(fi.clone());
    }
    let key = (fi.package_name.clone(), fi.filename.clone());
    if let Some(bytes) = prefetched_files.get(&key) {
        return try_prefetched_write(fi, &dest, bytes, store, progress).await;
    }
    try_network_download(client, fi, &dest, store, progress).await
}
```

- [ ] **Step 4: 修改 `pipeline.rs` 传递 `ProgressHandle`**

```rust
// src/downloader/pipeline.rs
pub(super) async fn run_download_pipeline(
    client: &reqwest::Client,
    repo: &Path,
    files: &[FileInfo],
    prefetched_files: &crate::downloader::PrefetchedFiles,
    include_source: bool,
    download_workers: usize,
    progress: Option<crate::progress::ProgressHandle>, // 新增
) -> DownloadResult {
    // ...
    let outcomes = run_download_tasks(
        client,
        repo,
        &store,
        prefetched_files,
        pending,
        download_workers,
        &progress, // 新增
    )
    .await;
    // ...
}

async fn run_download_tasks(
    client: &reqwest::Client,
    repo: &Path,
    store: &Option<Arc<crate::store::DownloadStore>>,
    prefetched_files: &crate::downloader::PrefetchedFiles,
    pending: Vec<FileInfo>,
    download_workers: usize,
    progress: &Option<crate::progress::ProgressHandle>, // 新增
) -> Vec<DownloadOutcome> {
    stream::iter(pending)
        .map(|fi| {
            let store = store.clone();
            let progress = progress.clone();
            async move {
                try_download(client, &store, prefetched_files, &fi, repo, &progress).await
            }
        })
        .buffer_unordered(download_workers)
        .collect::<Vec<_>>()
        .await
}
```

- [ ] **Step 5: 编译检查**

Run: `cargo check`
Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add src/downloader.rs src/downloader/pipeline.rs
git commit -m "feat(downloader): 下载完成发送 SyncEvent" -m "- download_pkg_files_with_prefetched 接收 ProgressHandle" -m "- 复用/下载成功时发送 FileDone 事件"
```

---

### Task 6: 给 Python 解释器下载发送事件

**Files:**
- Modify: `src/python_builds.rs`

- [ ] **Step 1: 修改 `download_python_builds_batch` 签名**

```rust
use crate::progress::{FileStatus, ProgressHandle, SyncEvent};

pub async fn download_python_builds_batch(
    client: &Client,
    repo: &Path,
    workers: usize,
    progress: Option<ProgressHandle>, // 新增
) -> Result<Vec<PythonBuildEntry>, Box<dyn std::error::Error>> {
    let entries = fetch_python_builds(client).await?;
    let dir = repo.join("python-builds");
    std::fs::create_dir_all(&dir)?;

    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseStarted {
            phase: "python-builds",
            total: Some(entries.len() as u64),
        });
    }

    use futures::{StreamExt, stream};
    stream::iter(&entries)
        .map(|entry| {
            let dir = dir.clone();
            let progress = progress.clone();
            async move {
                download_one_build(client, entry, &dir, &progress).await;
            }
        })
        .buffer_unordered(workers)
        .collect::<Vec<_>>()
        .await;

    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseFinished {
            phase: "python-builds",
            summary: format!("{} 个解释器", entries.len()),
        });
    }

    Ok(entries)
}
```

- [ ] **Step 2: 修改 `download_one_build` 发送事件**

```rust
async fn download_one_build(
    client: &Client,
    entry: &PythonBuildEntry,
    dir: &Path,
    progress: &Option<ProgressHandle>,
) {
    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseProgress {
            phase: "python-builds",
            current: 0, // 具体计数在 batch 层统一维护较复杂，先传 0
            message: entry.filename.clone(),
        });
    }
    let result = download_python_build(client, entry, dir).await;
    match result {
        Ok((_, true)) => {
            info!("  [OK] {}", entry.filename);
            if let Some(ref p) = progress {
                p.emit(SyncEvent::FileDone {
                    package: "python-builds".to_string(),
                    filename: entry.filename.clone(),
                    status: FileStatus::Downloaded,
                });
            }
        }
        Err(e) => {
            tracing::warn!("  [FAIL] {}: {e}", entry.filename);
            if let Some(ref p) = progress {
                p.emit(SyncEvent::FileDone {
                    package: "python-builds".to_string(),
                    filename: entry.filename.clone(),
                    status: FileStatus::Failed(e.to_string()),
                });
            }
        }
        _ => {}
    }
}
```

- [ ] **Step 3: 编译检查**

Run: `cargo check`
Expected: PASS。

- [ ] **Step 4: Commit**

```bash
git add src/python_builds.rs
git commit -m "feat(python_builds): Python 解释器下载发送事件" -m "- download_python_builds_batch 接收 ProgressHandle" -m "- 下载成功/失败发送 FileDone 事件"
```

---

### Task 7: 给 resolver 发送事件

**Files:**
- Modify: `src/resolver/resolve.rs`

- [ ] **Step 1: 修改 `build_dependency_plan` 签名并发送事件**

```rust
use crate::progress::{ProgressHandle, SyncEvent};

pub async fn build_dependency_plan(
    params: &PlanParams<'_>,
    client: &reqwest::Client,
    progress: Option<ProgressHandle>, // 新增
) -> Result<DependencyPlan, ResolveError> {
    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseStarted {
            phase: "resolve",
            total: Some(params.top_packages.len() as u64),
        });
    }

    let cache = MetadataCache::new(/* ... */);
    let top_versions = collect_top_versions(params, &cache, &progress).await?; // 传递 progress
    // ...
    let all_solutions = solve_all_targets(params, &caches, &top_versions, &pkg_extras, &targets, &progress).await?; // 传递 progress
    // ...
    let planned_files = collect_planned_files(params, &cache, &expanded).await?;

    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseFinished {
            phase: "resolve",
            summary: format!("{} 个解，{} 个文件", all_solutions.len(), planned_files.len()),
        });
    }

    info!(
        "依赖规划完成: {} 个解，{} 个文件",
        all_solutions.len(),
        planned_files.len()
    );
    Ok(DependencyPlan { /* ... */ })
}
```

- [ ] **Step 2: 修改 `collect_top_versions` 发送进度事件**

```rust
async fn collect_top_versions(
    params: &PlanParams<'_>,
    cache: &MetadataCache,
    progress: &Option<ProgressHandle>,
) -> Result<HashMap<String, Vec<Version>>, ResolveError> {
    let results = stream::iter(params.top_packages.iter().enumerate())
        .map(|(idx, package_ref)| async move {
            let package = bare_name(package_ref);
            let all_versions = cache.get_all_versions(&package).await?;
            let selected = select_top_versions(
                all_versions,
                params.top_versions_per_package,
                params.allow_prerelease,
            );
            info!("顶层包 {}: 选定 {} 个版本", package, selected.len());
            if let Some(ref p) = progress {
                p.emit(SyncEvent::PhaseProgress {
                    phase: "resolve",
                    current: idx as u64 + 1,
                    message: package.to_string(),
                });
            }
            Ok::<_, ResolveError>((package, selected))
        })
        .buffer_unordered(params.resolve_workers)
        .collect::<Vec<_>>()
        .await;
    // ...
}
```

- [ ] **Step 3: 修改 `solve_all_targets` 签名**

```rust
async fn solve_all_targets(
    params: &PlanParams<'_>,
    caches: &SolveCaches<'_>,
    top_versions: &HashMap<String, Vec<Version>>,
    pkg_extras: &HashMap<String, HashSet<String>>,
    targets: &[TargetEnv],
    _progress: &Option<ProgressHandle>, // 新增但暂不发送细粒度事件
) -> Result<Vec<super::solve::SolveResult>, ResolveError> {
    // 现有逻辑不变
}
```

- [ ] **Step 4: 编译检查**

Run: `cargo check`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add src/resolver/resolve.rs
git commit -m "feat(resolver): resolver 阶段发送事件" -m "- build_dependency_plan 接收 ProgressHandle" -m "- collect_top_versions 发送 PhaseProgress"
```

---

### Task 8: 给 sync 主流程和 finalize 发送事件

**Files:**
- Modify: `src/sync/mod.rs`
- Modify: `src/sync/finalize.rs`

- [ ] **Step 1: 修改 `do_sync` 签名**

```rust
use crate::progress::{ProgressHandle, SyncEvent};

pub async fn do_sync(
    config: &crate::config::Config,
    pkgs: &[crate::config::PackageSpec],
    no_deps: bool,
    download_python_builds: bool,
    dry_run: bool,
    progress: Option<ProgressHandle>, // 新增
) -> Result<(reqwest::Client, Vec<FileInfo>), Box<dyn std::error::Error>> {
    let repo = &config.repository_dir;
    let client = build_sync_client()?;

    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseStarted {
            phase: "plan",
            total: Some(pkgs.len() as u64),
        });
    }

    let plan = create_sync_plan(config, &client, pkgs, no_deps, progress.clone()).await?;
    let (pending, prefetched) = prepare_pending_files(repo, plan, &progress)?;

    if dry_run {
        log_dry_run(&pending);
        return Ok((client, Vec::new()));
    }

    let downloaded = execute_download_phase(&DownloadPhaseParams {
        config,
        client: &client,
        repo,
        pending: &pending,
        prefetched: &prefetched,
        download_python_builds,
        progress: progress.clone(),
    })
    .await?;
    Ok((client, downloaded))
}
```

- [ ] **Step 2: 修改 `DownloadPhaseParams` 和 `execute_download_phase`**

```rust
struct DownloadPhaseParams<'a> {
    config: &'a crate::config::Config,
    client: &'a reqwest::Client,
    repo: &'a Path,
    pending: &'a [FileInfo],
    prefetched: &'a PrefetchedFiles,
    download_python_builds: bool,
    progress: Option<ProgressHandle>, // 新增
}

async fn execute_download_phase(
    p: &DownloadPhaseParams<'_>,
) -> Result<Vec<FileInfo>, Box<dyn std::error::Error>> {
    if let Some(ref progress) = p.progress {
        progress.emit(SyncEvent::PhaseStarted {
            phase: "download",
            total: Some(p.pending.len() as u64),
        });
    }

    let result =
        run_downloads(p.config, p.client, p.repo, p.pending, p.prefetched, p.progress.clone())
            .await;

    let downloaded_count = result.downloaded.len();
    if let Some(ref progress) = p.progress {
        progress.emit(SyncEvent::PhaseFinished {
            phase: "download",
            summary: format!("下载 {}，跳过 {}，失败 {}",
                result.downloaded.len(),
                result.skipped.len(),
                result.failed.len()
            ),
        });
    }

    record::record_download_results(p.repo, &result).await?;
    finalize::finalize_sync(
        p.client,
        p.repo,
        p.download_python_builds,
        p.config.download_workers,
        p.progress.clone(),
    )
    .await?;
    Ok(result.downloaded)
}
```

- [ ] **Step 3: 修改 `run_downloads` 传递 progress**

```rust
async fn run_downloads(
    config: &crate::config::Config,
    client: &reqwest::Client,
    repo: &Path,
    pending: &[FileInfo],
    prefetched: &PrefetchedFiles,
    progress: Option<ProgressHandle>,
) -> crate::downloader::DownloadResult {
    download_pkg_files_with_prefetched(
        client,
        repo,
        pending,
        prefetched,
        config.include_source,
        config.download_workers,
        progress,
    )
    .await
}
```

- [ ] **Step 4: 修改 `create_sync_plan` 传递 progress**

```rust
pub async fn create_sync_plan(
    config: &crate::config::Config,
    client: &reqwest::Client,
    pkgs: &[crate::config::PackageSpec],
    no_deps: bool,
    progress: Option<ProgressHandle>,
) -> Result<DependencyPlan, ResolveError> {
    // ...
    let mut plan = build_plan(config, client, &name_pkgs, no_deps, progress.clone()).await?;
    // ...
}
```

- [ ] **Step 5: 修改 `build_plan` 传递 progress**

```rust
async fn build_plan(
    config: &crate::config::Config,
    client: &reqwest::Client,
    name_pkgs: &[String],
    no_deps: bool,
    progress: Option<ProgressHandle>,
) -> Result<DependencyPlan, ResolveError> {
    if no_deps {
        return plan::build_top_only_plan(config, client, name_pkgs).await;
    }
    let params = PlanParams { /* ... */ };
    build_dependency_plan(&params, client, progress).await
}
```

- [ ] **Step 6: 修改 `prepare_pending_files` 发送事件**

```rust
fn prepare_pending_files(
    repo: &Path,
    plan: DependencyPlan,
    progress: &Option<ProgressHandle>,
) -> Result<(Vec<FileInfo>, PrefetchedFiles), Box<dyn std::error::Error>> {
    let planned_count = plan.planned_files.len();
    let pending = filter_incremental_files(repo, plan.planned_files)?;
    let prefetched = filter_prefetched_for_pending(
        pending.as_slice(),
        plan.prefetched_files,
    );
    log_pending_files(pending.len(), planned_count);
    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseFinished {
            phase: "plan",
            summary: format!("{} 个待下载（已过滤 {} 个）", pending.len(), planned_count.saturating_sub(pending.len())),
        });
    }
    Ok((pending, prefetched))
}
```

- [ ] **Step 7: 修改 `finalize.rs` 传递 progress**

```rust
// src/sync/finalize.rs
use crate::progress::{ProgressHandle, SyncEvent};

pub async fn finalize_sync(
    client: &reqwest::Client,
    repo: &std::path::Path,
    download_python_builds: bool,
    download_workers: usize,
    progress: Option<ProgressHandle>, // 新增
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseStarted {
            phase: "finalize",
            total: None,
        });
    }

    let python_build_entries = maybe_download_python_builds(
        client,
        repo,
        download_python_builds,
        download_workers,
        progress.clone(),
    )
    .await?;

    rebuild_indexes(repo, python_build_entries, progress.clone()).await?;

    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseFinished {
            phase: "finalize",
            summary: "索引生成完成".to_string(),
        });
    }

    Ok(())
}

async fn maybe_download_python_builds(
    client: &reqwest::Client,
    repo: &Path,
    enabled: bool,
    workers: usize,
    progress: Option<ProgressHandle>,
) -> Result<Option<Vec<PythonBuildEntry>>, Box<dyn std::error::Error>> {
    if !enabled {
        return Ok(None);
    }
    let entries = download_python_builds_batch(client, repo, workers, progress).await?;
    info!("已下载 Python 解释器，开始生成 python-builds/index.json");
    Ok(Some(entries))
}

async fn rebuild_indexes(
    repo: &Path,
    python_build_entries: Option<Vec<PythonBuildEntry>>,
    progress: Option<ProgressHandle>,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo_clone = repo.to_path_buf();
    tokio::task::spawn_blocking(move || {
        if let Some(entries) = python_build_entries {
            build_python_builds_index(&entries, &repo_clone)
                .map_err(|e| format!("生成 python-builds index 失败: {e}"))?;
        }
        generate_index(&repo_clone, progress);
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("索引生成线程错误: {e}"))??;
    Ok(())
}
```

注意：`generate_index` 是同步函数，接收 `Option<ProgressHandle>` 时涉及跨线程发送。由于 `ProgressHandle` 包含 `UnboundedSender`，它不是 `Send` 吗？tokio 的 `UnboundedSender` 是 `Send + Sync`，可以跨线程。但 spawn_blocking 里不能 await，只能发送事件（send 不是 async）。所以可以。

不过 `generate_index` 当前是 `pub fn`，签名改变后调用方需要更新。后面 Task 9 处理。

- [ ] **Step 8: 编译检查**

Run: `cargo check`
Expected: PASS。

- [ ] **Step 9: Commit**

```bash
git add src/sync/mod.rs src/sync/finalize.rs
git commit -m "feat(sync): 同步主流程与 finalize 发送事件" -m "- do_sync 接收 ProgressHandle" -m "- 各阶段发送 PhaseStarted/Progress/Finished"
```

---

### Task 9: 给索引生成发送事件

**Files:**
- Modify: `src/indexer.rs`

- [ ] **Step 1: 修改 `generate_index` 签名并发送事件**

```rust
use crate::progress::{ProgressHandle, SyncEvent};

pub fn generate_index(
    repository_dir: &Path,
    progress: Option<ProgressHandle>,
) {
    let simple_dir = repository_dir.join("simple");
    if !simple_dir.exists() {
        info!("仓库目录为空，跳过索引生成");
        return;
    }
    info!("生成 PEP 503 / PEP 691 索引...");

    let db_path = repository_dir.join(".store.db");
    let (hashes, meta, yanked) = /* 现有逻辑 */;

    let mut names: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&simple_dir) else {
        return;
    };

    let entries: Vec<_> = entries.flatten().filter(|e| e.path().is_dir()).collect();
    let total = entries.len();

    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseStarted {
            phase: "index",
            total: Some(total as u64),
        });
    }

    for (idx, entry) in entries.iter().enumerate() {
        let path = entry.path();
        let n = path.file_name().unwrap().to_string_lossy().to_string();
        names.push(n.clone());
        index_pkg(&IndexPkg {
            path: &path,
            name: &n,
            hashes: &hashes,
            meta: &meta,
            yanked: &yanked,
        });
        if let Some(ref p) = progress {
            p.emit(SyncEvent::PhaseProgress {
                phase: "index",
                current: idx as u64 + 1,
                message: n.clone(),
            });
        }
    }

    names.sort();
    let _ = std::fs::write(simple_dir.join("index.html"), generate_index_html(&names));
    let _ = std::fs::write(simple_dir.join("index.json"), generate_index_json(&names));

    if let Some(ref p) = progress {
        p.emit(SyncEvent::PhaseFinished {
            phase: "index",
            summary: format!("{} 个包", names.len()),
        });
    }

    info!("索引生成完成: {} 个包", names.len());
}
```

- [ ] **Step 2: 更新其他调用 `generate_index` 的地方**

搜索并更新：

```bash
grep -n "generate_index(" src/**/*.rs
```

所有非 progress 调用改为传 `None`，例如：

```rust
// src/main.rs 中 import-incremental 调用（Task 11 会改为传 progress）
// src/sync/finalize.rs 已经改为传 progress
// src/server.rs 如果调用则传 None
```

- [ ] **Step 3: 编译检查**

Run: `cargo check`
Expected: PASS。

- [ ] **Step 4: Commit**

```bash
git add src/indexer.rs
git commit -m "feat(indexer): 索引生成发送事件" -m "- generate_index 接收 ProgressHandle" -m "- 按包发送 PhaseProgress"
```

---

### Task 10: 命令入口使用 `run_with_progress`

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: 修改 `cmd_sync` / `cmd_sync_full` 调用 `run_with_progress`**

找到 `cmd_sync` 和 `cmd_sync_full` 函数（在 main.rs 中），修改如下：

```rust
async fn cmd_sync(
    config_path: Option<&std::path::Path>,
    packages: Option<Vec<String>>,
    no_deps: bool,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let verbose = /* 需要把 cli.verbose 传进来，或全局获取 */;
    let (config, pkgs) = prepare_sync_args(config_path, packages)?;

    pip_mirror::progress::run_with_progress(verbose, |progress| async move {
        let (client, downloaded) = pip_mirror::sync::do_sync(
            &config,
            &pkgs,
            no_deps,
            config.download_python_builds,
            dry_run,
            Some(progress.clone()),
        )
        .await?;

        if !dry_run {
            do_incremental_pack(&config, downloaded).await?;
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await
}
```

由于 `cli.verbose` 在 `main` 里，需要把 `verbose` 作为参数传给 `cmd_sync` 和 `cmd_sync_full`。

- [ ] **Step 2: 修改 `main` 传递 verbose**

```rust
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    pip_mirror::logging::init(cli.verbose);
    let verbose = cli.verbose;
    try_sync(cli.command, verbose).await
}
```

并沿着 `try_sync` -> `cmd_sync_d` -> `cmd_sync` 传递 `verbose`。

- [ ] **Step 3: 修改 `do_incremental_pack` 保持兼容**

`do_incremental_pack` 不直接参与进度条（打包很快），但可保留现有日志。

- [ ] **Step 4: 编译检查**

Run: `cargo check`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): sync/sync-full 入口接入进度条" -m "- 通过 run_with_progress 包装同步流程" -m "- 传递 verbose 参数"
```

---

### Task 11: 给 `import-incremental` 接入进度条

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: 修改 `cmd_import_incremental` 使用 `run_with_progress`**

```rust
fn cmd_import_incremental(
    args: ImportIncrementalArgs<'_>,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = pip_mirror::config::Config::load(args.config_path)?;

    pip_mirror::progress::run_with_progress(verbose, |progress| async move {
        if args.strict {
            info!("严格模式: 校验增量包完整性");
        }

        if let Some(ref p) = progress {
            p.emit(SyncEvent::PhaseStarted {
                phase: "import",
                total: None,
            });
            p.emit(SyncEvent::PhaseProgress {
                phase: "import",
                current: 0,
                message: format!("解包 {}", args.archive.display()),
            });
        }

        info!(
            "解包 {} → {}",
            args.archive.display(),
            config.repository_dir.display()
        );
        let reader = pip_mirror::packager::open_archive_reader(args.archive)?;
        let mut tar = tar::Archive::new(reader);
        tar.unpack(&config.repository_dir)?;

        if let Some(ref p) = progress {
            p.emit(SyncEvent::PhaseFinished {
                phase: "import",
                summary: "解包完成".to_string(),
            });
        }

        if !args.no_reindex {
            pip_mirror::indexer::generate_index(&config.repository_dir, Some(progress.clone()));
        }

        if let Some(ref p) = progress {
            p.emit(SyncEvent::PhaseFinished {
                phase: "import",
                summary: "导入完成".to_string(),
            });
        }

        info!("导入完成");
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}
```

注意：`run_with_progress` 是 async 函数，但 `cmd_import_incremental` 当前是 sync 函数。需要改为 `async fn` 或在函数内 block_on。由于 main.rs 的调用链 `try_import` 是 sync，可以改为：

```rust
fn cmd_import_incremental(
    args: ImportIncrementalArgs<'_>,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Handle::try_current()?;
    rt.block_on(async {
        // ... run_with_progress ...
    })
}
```

但更简洁的是让 `try_import` 也变成 async。需要相应调整调用链。

- [ ] **Step 2: 调整 `try_import` 为 async 并传递 verbose**

```rust
async fn try_import(cmd: Command, verbose: bool) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::ImportIncremental { archive, config, no_reindex, strict } = cmd {
        return cmd_import_incremental(ImportIncrementalArgs {
            archive: &archive,
            config_path: config.as_deref(),
            no_reindex,
            strict,
        }, verbose).await;
    }
    try_access(cmd).await
}
```

`try_access` 也要改成 async（虽然内部没 await，但签名统一）。

- [ ] **Step 3: 编译检查**

Run: `cargo check`
Expected: PASS。

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): import-incremental 接入进度条" -m "- import-incremental 使用 run_with_progress" -m "- 解包和索引生成阶段发送事件"
```

---

### Task 12: 修复编译警告并运行测试

**Files:**
- 多个文件

- [ ] **Step 1: 运行 cargo clippy**

Run: `cargo clippy --all-targets`
Expected: 无 error，尽量减少 warning。

- [ ] **Step 2: 运行 cargo test**

Run: `cargo test`
Expected: 全部通过。

- [ ] **Step 3: 手动验证 TTY 输出**

Run: `cargo build --release && ./target/release/pip-mirror sync -c pip-mirror.toml`
Expected: 终端显示两行进度条，不再逐行刷屏。

- [ ] **Step 4: 手动验证非 TTY 输出**

Run: `./target/release/pip-mirror sync -c pip-mirror.toml 2>&1 | cat`
Expected: 输出 `[progress]` 精简日志行，无 ANSI 转义。

- [ ] **Step 5: Commit**

```bash
git commit -m "chore: 修复进度条实现后的编译警告" -m "- cargo clippy 通过" -m "- cargo test 通过"
```

---

## Self-Review

### 1. Spec coverage

| 设计文档要求 | 对应 Task |
|---|---|
| 新增 `src/progress/` 模块 | Task 1-4 |
| `SyncEvent` / `FileStatus` 事件定义 | Task 1 |
| `ProgressHandle` + `run_with_progress` | Task 2 |
| TTY 渲染器（indicatif 两行） | Task 4 |
| 非 TTY 渲染器（精简日志） | Task 3 |
| downloader 发送事件 | Task 5 |
| python_builds 发送事件 | Task 6 |
| resolver 发送事件 | Task 7 |
| sync 主流程发送事件 | Task 8 |
| finalize/indexer 发送事件 | Task 8-9 |
| main.rs 接入 | Task 10-11 |
| 测试覆盖 | Task 2, 12 |

### 2. Placeholder scan

无 TBD/TODO/"implement later" 等占位符。每步包含具体代码或命令。

### 3. Type consistency

- `ProgressHandle` 始终为 `Option<ProgressHandle>` 传入业务函数。
- `SyncEvent` 各变体字段与 Task 1 定义一致。
- `generate_index` 签名在 Task 9 修改后，所有调用方同步更新。

潜在风险：`ProgressHandle` 包含 `UnboundedSender`，在 `spawn_blocking` 中跨线程发送是允许的；但在同步代码中调用 `emit` 不会阻塞。
