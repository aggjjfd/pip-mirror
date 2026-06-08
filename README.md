# pip-mirror

面向内外网迁移的轻量私有 PyPI 镜像：  
外网机负责下载，内网机负责服务，客户端同时支持 `pip install` 与 `uv python install`。

## 生命周期（全量 + 增量）

```mermaid
flowchart LR
    A[外网机: 配置 packages] --> B[sync-full]
    B --> C[mirror.tar.gz + mirror.sha256]
    C --> D[拷贝到内网机]
    D --> E[验签并解压到 packages/]
    E --> F[serve/systemd 启动服务]
    F --> G[内网客户端 pip/uv 使用]
    G --> H[外网机日常 sync]
    H --> I[incremental_*.tar.gz]
    I --> J[拷贝到内网机]
    J --> K[import-incremental]
    K --> F
```

## 0. 准备

```bash
git clone https://github.com/aggjjfd/pip-mirror.git
cd pip-mirror
cargo build --release
./target/x86_64-unknown-linux-musl/release/pip-mirror --help
```

生成配置模板：

```bash
./target/x86_64-unknown-linux-musl/release/pip-mirror init -o pip-mirror.toml
```

最小配置示例（按需改包名）：

```toml
packages = ["requests", "openai", "playwright"]
repository_dir = "./packages"
incremental_dir = "./incremental"
include_source = true
resolve_workers = 8
metadata_workers = 32
download_workers = 8
server_host = "0.0.0.0"
server_port = 8080
```

说明：当前解释器镜像以 CPython 3.8-3.12 为主，默认不覆盖 3.13/3.14。`include_source = true` 时仅在无可用 wheel 且判定为纯 Python 回退条件满足时才会保留源码包；否则 warning 后跳过。

也支持直接指定 whl URL（会读取其 METADATA 并参与依赖解析，等价于把它声明为顶层包）：

```toml
packages = [
    "requests",
    { url = "https://example.com/foo-1.0-py3-none-any.whl" },
    { url = "file:///opt/wheels/bar-1.0-py3-none-any.whl", sha256 = "abc..." },
]
```

## 1. 外网机首次全量下载

```bash
./target/x86_64-unknown-linux-musl/release/pip-mirror sync-full -c pip-mirror.toml
```

产物：`mirror.tar.gz` + `mirror.sha256`。用途：首次上线、灾备重建、基线重置。

## 2. 内网机落地全量数据并启动服务

把上一步两个文件拷到内网机后执行：

```bash
sha256sum -c mirror.sha256
tar -xzf mirror.tar.gz
./target/x86_64-unknown-linux-musl/release/pip-mirror serve -c pip-mirror.toml
```

容器方式也可以：

```bash
docker compose up -d
```

`systemd` 常驻方式（推荐）：

```bash
sudo install -m 644 deploy/systemd/pip-mirror.service /etc/systemd/system/pip-mirror.service
sudo useradd --system --home /var/lib/pip-mirror --shell /usr/sbin/nologin pipmirror || true
sudo systemctl daemon-reload
sudo systemctl enable --now pip-mirror
sudo systemctl status pip-mirror --no-pager
```

默认 unit 使用：
- 二进制：`/usr/local/bin/pip-mirror`
- 配置：`/etc/pip-mirror/pip-mirror.toml`
- 数据目录：`/var/lib/pip-mirror`

## 3. 外网机日常增量下载

```bash
./target/x86_64-unknown-linux-musl/release/pip-mirror sync -c pip-mirror.toml
```

有新增时会生成 `incremental/incremental_*.tar.gz`；无新增则不产包。

## 4. 内网机导入增量包

把增量包拷到内网机后执行：

```bash
./target/x86_64-unknown-linux-musl/release/pip-mirror import-incremental \
  ./incremental_20260507_120000.tar.gz -c pip-mirror.toml
```

导入后会更新仓库与索引，服务无需重启即可对客户端可见。

## 5. 客户端使用（内网）

`uv`：

```bash
export UV_DEFAULT_INDEX=http://<内网IP>:8080/simple
export UV_PYTHON_DOWNLOADS_JSON_URL=http://<内网IP>:8080/python-builds/index.json
uv python install 3.12
uv pip install requests
```

`pip`：

```bash
pip config set global.index-url http://<内网IP>:8080/simple
pip config set global.trusted-host <内网IP>
pip install requests
```

## 6. 推荐运行节奏

1. 首次：`sync-full` -> 传输 `mirror.tar.gz` -> 内网 `serve`。
2. 日常：外网 `sync` -> 传输 `incremental_*.tar.gz` -> 内网 `import-incremental`。
3. 大版本变更或灾备：重新走一次 `sync-full` 全量链路。

## 7. 常用命令

```bash
./target/x86_64-unknown-linux-musl/release/pip-mirror sync-full -c pip-mirror.toml

./target/x86_64-unknown-linux-musl/release/pip-mirror sync -c pip-mirror.toml

./target/x86_64-unknown-linux-musl/release/pip-mirror import-incremental <incremental.tar.gz> -c pip-mirror.toml

./target/x86_64-unknown-linux-musl/release/pip-mirror serve -c pip-mirror.toml

./target/x86_64-unknown-linux-musl/release/pip-mirror access-log -n 30
```
