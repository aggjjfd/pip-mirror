# pip-mirror 项目约定

## type_state_builder

用 `type_state_builder` crate 来替代参数过多的函数签名。当函数参数超过 5 个时，将参数分组为 struct 并用 `#[derive(TypeStateBuilder)]` 生成 builder。

用法：
- `#[builder(impl_into)]` — 所有 setter 接受 `impl Into<T>`，传引用时需注意 `Arc` 需要用 `&*` 解引用
- `#[builder(required)]` — 标记必需字段，`build()` 在所有 required 字段设置后才可用
- `#[builder(build_method = "...")]` — 自定义 `build()` 方法名
- `#[builder(default = expr)]` — 提供默认值

文档: <https://docs.rs/type_state_builder/latest/type_state_builder/>

## pre-commit 合规

本项目的 `.pre-commit-config.yaml` 是预配置的硬性门禁，所有修改必须通过其全部检查。

严禁以下行为：
- **`git commit --no-verify` / `-n`** — 禁止跳过 hook
- **`#[allow(clippy::...)]`** — 禁止用 allow 注释抑制 clippy 警告
- **任何其他逃避 pre-commit 检查的手段**

当 hook 报错时，必须修复根本问题（重构代码满足门禁要求），而不是绕过检查。参考 `type_state_builder` 和参数合并等模式来满足 clippy 规则。

降低复杂度必须只用结构手段：拆分模块、提取函数、简化分支。禁止压缩代码排版（合并多行签名、压扁链式调用、删除空行等）来通过 NLOC 或 Score 检查——`cargo fmt` 会展开一切，导致反弹。正确的做法：
- 将大文件拆成独立模块（如 `solve_cache.rs`）
- 将复杂函数拆成多个小函数
- 简化条件分支逻辑
- 这些不会和 formatter 冲突
