# 进度条终端宽度自适应实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `pip-mirror` 的 TTY 进度条根据终端窗口宽度实时调整消息区与进度条布局。

**Architecture:** 在 `src/progress/tty.rs` 内新增 `Layout` 与纯函数 `calculate_layout`，渲染循环通过 `tokio::select!` 同时监听 `SyncEvent` 和 200ms 轮询终端宽度；宽度变化时重新生成 `ProgressStyle` 并应用。

**Tech Stack:** Rust, tokio, indicatif, console (indicatif 已依赖)

---

## 文件变更

- **修改：** `src/progress/tty.rs` — 所有改动集中在此文件。

---

### Task 1: 新增 `Layout` 与 `calculate_layout`

**Files:**
- Modify: `src/progress/tty.rs`

- [ ] **Step 1: 在常量区下方添加布局类型与计算函数**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    msg_width: usize,
    bar_width: usize,
}

const MIN_MSG_LEN: usize = 8;
const DEFAULT_BAR_WIDTH: usize = 25;
const MIN_BAR_WIDTH: usize = 5;
const POS_LEN_WIDTH: usize = 12;
const MIN_TOTAL_WIDTH: usize = 30;
const FALLBACK_WIDTH: u16 = 80;

fn calculate_layout(total_cols: u16) -> Layout {
    if total_cols < MIN_TOTAL_WIDTH {
        return Layout {
            msg_width: MIN_MSG_LEN,
            bar_width: MIN_BAR_WIDTH,
        };
    }

    let available = total_cols.saturating_sub(POS_LEN_WIDTH) as usize;
    let msg_width = available.saturating_sub(DEFAULT_BAR_WIDTH).max(MIN_MSG_LEN);

    Layout {
        msg_width,
        bar_width: DEFAULT_BAR_WIDTH,
    }
}
```

- [ ] **Step 2: 在文件底部添加单元测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_layout_wide() {
        let layout = calculate_layout(120);
        assert_eq!(layout.msg_width, 120 - POS_LEN_WIDTH - DEFAULT_BAR_WIDTH);
        assert_eq!(layout.bar_width, DEFAULT_BAR_WIDTH);
    }

    #[test]
    fn test_calculate_layout_normal() {
        let layout = calculate_layout(80);
        assert_eq!(layout.msg_width, 80 - POS_LEN_WIDTH - DEFAULT_BAR_WIDTH);
        assert_eq!(layout.bar_width, DEFAULT_BAR_WIDTH);
    }

    #[test]
    fn test_calculate_layout_narrow() {
        let layout = calculate_layout(40);
        assert_eq!(layout.msg_width, MIN_MSG_LEN);
        assert_eq!(layout.bar_width, DEFAULT_BAR_WIDTH);
    }

    #[test]
    fn test_calculate_layout_minimum() {
        let layout = calculate_layout(20);
        assert_eq!(layout.msg_width, MIN_MSG_LEN);
        assert_eq!(layout.bar_width, MIN_BAR_WIDTH);
    }
}
```

- [ ] **Step 3: 运行测试确认失败/通过**

Run: `cargo test --lib progress::tty::tests`
Expected: PASS（如果先写代码再写测试，则应为 PASS）

- [ ] **Step 4: 提交**

```bash
git add src/progress/tty.rs
git commit -m "feat(progress): 增加终端宽度自适应布局计算与测试"
```

---

### Task 2: 渲染循环实时响应终端宽度变化

**Files:**
- Modify: `src/progress/tty.rs`

- [ ] **Step 1: 引入 `console` 并改造 `render`**

```rust
use console::Term;
use tokio::time::{Duration, interval};
```

```rust
pub async fn render(mut rx: UnboundedReceiver<SyncEvent>) {
    let multi = MultiProgress::with_draw_target(
        ProgressDrawTarget::stderr_with_hz(MAX_FPS),
    );
    super::set_progress_multi(multi.clone());
    let bar = multi.add(ProgressBar::new(0));

    let mut phase_totals: HashMap<&'static str, u64> = HashMap::new();
    let mut state = RenderState {
        bar: &bar,
        phase_totals: &mut phase_totals,
    };

    let term = Term::stderr();
    let mut last_layout = apply_layout(&bar, calculate_layout(term.size().1));
    let mut resize_tick = interval(Duration::from_millis(200));

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Some(event) => process_event(&multi, &mut state, event),
                    None => break,
                }
            }
            _ = resize_tick.tick() => {
                let new_layout = calculate_layout(term.size().1);
                if new_layout != last_layout {
                    apply_layout(&bar, new_layout);
                    last_layout = new_layout;
                }
            }
        }
    }

    bar.finish();
}
```

- [ ] **Step 2: 新增 `apply_layout` 辅助函数**

```rust
fn apply_layout(bar: &ProgressBar, layout: Layout) -> Layout {
    bar.set_style(
        ProgressStyle::with_template(&format!(
            "{{msg:<{}}} [{{bar:{}}}] {{pos}}/{{len}}",
            layout.msg_width, layout.bar_width
        ))
        .unwrap()
        .progress_chars("##-"),
    );
    layout
}
```

- [ ] **Step 3: 新增/改造 `process_event` 以替代内联 `process_events`**

```rust
fn process_event(
    multi: &MultiProgress,
    state: &mut RenderState<'_>,
    event: SyncEvent,
) {
    match event {
        SyncEvent::PhaseStarted { phase, total } => {
            handle_phase_started(state, phase, total);
        }
        SyncEvent::PhaseProgress {
            phase,
            current,
            message,
        } => {
            handle_phase_progress(state, phase, current, message);
        }
        SyncEvent::PhaseFinished { phase, summary } => {
            handle_phase_finished(state, phase, summary);
        }
        SyncEvent::FileDone {
            filename,
            status: s,
            ..
        } => {
            handle_file_done(multi, filename, s);
        }
        SyncEvent::Error { message } | SyncEvent::Warning { message } => {
            multi.println(format!("! {message}")).ok();
        }
    }
}
```

- [ ] **Step 4: 删除旧的 `process_events` 函数**

确认 `src/progress/tty.rs` 中不再存在 `process_events`。

- [ ] **Step 5: 删除已废弃的常量 `MAX_MSG_LEN` 和 `BAR_WIDTH`**

```rust
// 删除以下两行
const MAX_MSG_LEN: usize = 30;
const BAR_WIDTH: usize = 25;
```

- [ ] **Step 6: 运行 fmt、clippy、测试、复杂度门禁**

Run:
```bash
cargo fmt
cargo clippy --all-targets
cargo test --lib
python scripts/check-complexity.py src/progress/tty.rs
```
Expected: 全部通过。

- [ ] **Step 7: 手动验证**

在真实终端中运行一次 `sync` 或 `sync-full`，拖放窗口宽度，确认消息区和进度条变化。

- [ ] **Step 8: 提交**

```bash
git add src/progress/tty.rs
git commit -m "feat(progress): 进度条随终端宽度实时自适应"
```

---

## 自检

- [x] Spec 覆盖：自适应算法（Task 1）、实时检测（Task 2）、最小布局（Task 1）、测试（Task 1/2）均已对应任务。
- [x] 无占位符：每个步骤包含完整代码或命令。
- [x] 类型一致：`Layout` 在 Task 1 定义，Task 2 使用；`apply_layout` 返回 `Layout` 用于比较。
