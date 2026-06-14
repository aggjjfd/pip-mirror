# 进度条随终端宽度自适应缩放

## 背景

`pip-mirror` 在 TTY 模式下使用单条进度条展示同步阶段。当前实现硬编码了消息区宽度（30）和进度条宽度（25），在窄终端里消息容易被截断，在宽终端里又浪费空间。本设计让进度条根据当前终端宽度动态调整布局。

## 目标

- 进度条在 TTY 模式下随窗口宽度实时缩放。
- 优先保证消息区可读：终端越宽，消息区越长；进度条宽度保持固定。
- 终端宽度不足时保留最小布局，不崩溃、不换行。
- 无法获取终端宽度时提供固定兜底。

## 非目标

- 不支持非 TTY 模式（继续使用现有固定宽度）。
- 不改多行/多进度条结构（当前仍只有单条主进度条）。
- 不处理字体/字符宽度异常（如双字节字符），仅按列数计算。

## 设计

### 布局算法

每行固定占用以下空间：

```text
[msg_width] [bar_width] [pos/len]
```

其中 `pos/len` 预留 12 列（如 ` 99999/99999`）。实际计算：

```rust
const MIN_MSG_LEN: usize = 8;
const DEFAULT_BAR_WIDTH: usize = 25;
const MIN_BAR_WIDTH: usize = 5;
const POS_LEN_WIDTH: usize = 12;
const MIN_TOTAL_WIDTH: usize = 30;

fn calculate_layout(total_cols: u16) -> Layout {
    if total_cols < MIN_TOTAL_WIDTH {
        return Layout {
            msg_width: MIN_MSG_LEN,
            bar_width: MIN_BAR_WIDTH,
        };
    }

    let available = total_cols.saturating_sub(POS_LEN_WIDTH);
    let msg_width = available.saturating_sub(DEFAULT_BAR_WIDTH) as usize;
    let msg_width = msg_width.max(MIN_MSG_LEN);

    Layout {
        msg_width,
        bar_width: DEFAULT_BAR_WIDTH,
    }
}
```

### 实时检测

- 在 `src/progress/tty.rs` 的 `render()` 中使用 `tokio::select!` 同时监听 `SyncEvent` 通道和一个 200ms 的 `tokio::time::interval`。
- 每次 tick 调用 `console::Term::stderr().size()` 读取终端列数。
- 列数变化超过 0 时，重新生成 `ProgressStyle` 并通过 `bar.set_style(...)` 应用。
- 复用 `indicatif` 已依赖的 `console` crate，不新增依赖。

### 错误与兜底

- 读取终端尺寸失败：沿用上一次有效宽度；首次失败使用 80 列。
- 宽度为 0 或大于 1000：视为无效，保持上一次/兜底宽度。

### 最小布局

当终端宽度小于 30 列时：

- 消息区固定 8 字符并截断。
- 进度条固定 5 字符。
- 仍显示 `pos/len`。

## 影响范围

- 仅修改 `src/progress/tty.rs`。
- 新增纯函数 `calculate_layout` 便于单元测试。

## 测试

1. 单元测试 `calculate_layout`：覆盖 200/120/80/40/20/10 列，验证 `msg_width` 和 `bar_width`。
2. 手动验证：在真实终端中拖放窗口，观察消息区和进度条变化。

## 验收标准

- 在 80 列终端中，消息区可显示约 43 字符。
- 在 40 列终端中，消息区被压缩但不会导致换行或异常输出。
- 窗口缩放时 200ms 内完成布局更新。
- `cargo clippy`、`cargo test`、`check-complexity` 均通过。
