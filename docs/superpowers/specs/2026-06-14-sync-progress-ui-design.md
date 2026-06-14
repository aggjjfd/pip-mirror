# pip-mirror 同步命令进度条 UI 设计

## 背景

当前 `sync`、`sync-full`、`import-incremental` 命令使用 `tracing::info!` 逐行输出每个文件的下载/复用状态，以及 resolver 各阶段信息。当文件数量较多时，终端被大量重复日志刷屏，信噪比低，用户难以快速判断整体进度。

项目已经在 `Cargo.toml` 中依赖 `indicatif`，但尚未使用。

## 目标

为同步相关命令提供清晰的进度展示：

- 默认使用**一行进度条 + 一行当前状态**替换逐文件日志。
- 非 TTY 环境（CI、systemd、重定向）自动回退到精简日志行。
- 错误即时可见，阶段结束给出汇总。
- `--verbose` 仍保留逐行日志，便于排查。

## 范围

覆盖命令：

- `sync`
- `sync-full`
- `import-incremental`

不覆盖：

- `serve`（长效服务，使用 tracing 日志）
- `access-log`（结果列表，无需进度条）
- `init`（瞬间完成）

## 方案选择

采用**事件总线式**方案：

- 业务模块发送事件，不直接操作 UI。
- 独立渲染层消费事件，根据 TTY/非 TTY 选择展示方式。
- 相比全局最小改动式，解耦更好、可测试；相比集中式 Reporter，事件模型更利于未来扩展（如 Web UI、结构化日志）。

## 详细设计

### 模块结构

```
src/progress/
├── mod.rs      # 公共 API：run_with_progress、ProgressHandle
├── events.rs   # SyncEvent / FileStatus 定义
├── tty.rs      # TTY 渲染器：indicatif 两行进度条
└── plain.rs    # 非 TTY 渲染器：精简日志行
```

### 事件定义

```rust
pub enum FileStatus {
    Downloaded,
    Reused,
    Skipped,
    Failed(String),
}

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

### 公共 API

```rust
pub struct ProgressHandle {
    tx: mpsc::UnboundedSender<SyncEvent>,
}

impl ProgressHandle {
    pub fn emit(&self, event: SyncEvent) {
        let _ = self.tx.send(event); // 渲染任务崩溃不应拖垮同步
    }
}

pub async fn run_with_progress<F, Fut, T>(
    verbose: bool,
    f: F,
) -> Result<T, Box<dyn std::error::Error>>
where
    F: FnOnce(ProgressHandle) -> Fut,
    Fut: std::future::Future<Output = Result<T, Box<dyn std::error::Error>>>,
{
    // verbose 只控制 tracing 日志级别，不影响进度条渲染。
    if verbose {
        // 已在 logging::init 中设置，此处无需额外操作；保留参数用于显式表达语义。
    }

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = ProgressHandle { tx };

    let renderer = if std::io::stdout().is_terminal() {
        tty::render(rx)
    } else {
        plain::render(rx)
    };

    let result = f(handle).await;
    // handle 在这里 drop，关闭 sender，渲染任务收到结束信号后打印最终汇总。
    drop(handle);
    renderer.await;
    result
}
```

### 数据流

1. 命令入口调用 `progress::run_with_progress(verbose, |handle| async { ... })`。
2. `run_with_progress` 创建 channel，启动渲染任务。
3. 业务闭包拿到 `ProgressHandle`，传给 `do_sync`、`finalize_sync`、`download_pkg_files_with_prefetched`、`download_python_builds_batch` 等函数。
4. 各阶段函数在关键点发送事件。
5. 渲染任务消费事件并更新界面。
6. 业务闭包返回后，关闭 sender，渲染任务打印最终汇总并退出。

### TTY 渲染实现

使用 `indicatif::MultiProgress` 挂两个 `ProgressBar`：

- 第一行：阶段总体进度，样式为 `[{bar:40}] {pos}/{len} {msg}`。
- 第二行：当前操作状态，不显示进度条，只显示 `{msg}`，例如当前正在下载的文件名或 resolver 正在处理的包名。

`MultiProgress` 保证两行一起刷新，避免传统 `println!` 造成的刷屏。

### TTY 渲染效果

```
解析依赖 ████████████████████████████████████████  124/124  resolving requests
下载文件 ▓▓▓▓▓▓▓▓░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░   31/156  requests-2.32.0-py3...
```

错误出现时，在进度条下方追加一行：

```
! hash 校验失败: bad.whl
```

阶段切换时复用同一行或新增行，避免刷屏。

### 非 TTY 渲染效果

```
[progress] 解析依赖 124/124 完成
[progress] 下载文件 31/156 requests-2.32.0-py3...
[warn] hash 校验失败: bad.whl
[progress] 下载文件 156/156 完成，失败 1
```

### 错误处理

- 事件发送使用 `try_send`/`send` 并忽略失败，渲染任务崩溃不应影响同步流程。
- `Error` 事件立即渲染。
- `PhaseFinished` 携带阶段汇总。
- 严重错误（如 resolver 失败）直接返回 `Err`，由命令入口在进度条清理后打印。

### 与 tracing 的关系

- 默认情况下，同步阶段的逐文件 `tracing::info!` 改为发送事件，不再直接输出。
- `--verbose` 时额外启用 `tracing::info!`，保留逐行排查能力。
- `tracing::warn!` 用于真正的异常情况，同时转发一份到事件系统，确保在进度条界面下也能看见。

## 测试策略

- `ProgressHandle` 可替换为测试用的 `Vec<SyncEvent>` 收集器，方便断言事件顺序和内容。
- 渲染器单独测试：给定事件序列，断言输出字符串或 ANSI 行为。
- 保持原有集成测试不变，只验证功能结果；进度条行为由单元测试覆盖。

## 验收标准

- [ ] `sync`、`sync-full`、`import-incremental` 在 TTY 下默认显示进度条 + 两行状态，不再逐行刷屏。
- [ ] 非 TTY 下自动回退到精简日志行，无 ANSI 转义残留。
- [ ] `--verbose` 模式下仍输出逐文件日志。
- [ ] 下载失败等错误即时显示，并在阶段结束时汇总。
- [ ] 原有集成测试全部通过。
- [ ] 新增 `progress` 模块单元测试覆盖事件收发和两种渲染器输出。
