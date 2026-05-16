# pip-mirror 项目约定

## type_state_builder

用 `type_state_builder` crate 来替代参数过多的函数签名。当函数参数超过 5 个时，将参数分组为 struct 并用 `#[derive(TypeStateBuilder)]` 生成 builder。

用法：
- `#[builder(impl_into)]` — 所有 setter 接受 `impl Into<T>`，传引用时需注意 `Arc` 需要用 `&*` 解引用
- `#[builder(required)]` — 标记必需字段，`build()` 在所有 required 字段设置后才可用
- `#[builder(build_method = "...")]` — 自定义 `build()` 方法名
- `#[builder(default = expr)]` — 提供默认值

文档: <https://docs.rs/type_state_builder/latest/type_state_builder/>
