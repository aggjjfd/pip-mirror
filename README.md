# pip-mirror

轻量级私有 PyPI 镜像。一台服务器同时供 `pip install` 与 `uv python install` 使用，适合内网离线部署。

特性

- 同步 PyPI 包并自动解析依赖
- 同步 `python-build-standalone` 解释器（uv 兼容）
- PEP 503 / PEP 658 / PEP 691 索引（含 sha256 与独立 metadata）
- SQLite 增量记录，跳过已下载文件
- 内置 HTTP server，支持反代 `X-Forwarded-For`，记录访问日志
- Rust 重写，单二进制文件，静态编译后体积约 10 MB

## 安装

需要 Rust 工具链（1.85+）：

```bash
git clone https://github.com/aggjjfd/pip-mirror.git
cd pip-mirror
cargo build --release
./target/release/pip-mirror --help
```

`cargo build --release` 产物为 `./target/release/pip-mirror`。Docker 镜像用 musl 静态编译 + scratch，同样约 10 MB。

## 四种使用模式

对外只有四种使用模式，每种模式都有清楚的单一命令、单一产物。请先确认自己处在哪种模式，再去看对应的章节。

- 模式一：增量更新 —— `pip-mirror sync`，产出 `incremental/incremental_*.tar.gz`。日常用，只下载 wheel 与 Python 解释器的新增部分。
- 模式二：全量更新 —— `pip-mirror sync-full`，产出仓库根目录的 `mirror.tar.gz` + `mirror.sha256`。首次部署或灾后重建用，会先清空仓库再重拉。
- 模式三：服务启动 —— `pip-mirror serve` 或 `docker compose up -d`。内网消费端用。
- 模式四：构建 Docker 镜像 —— `docker build -t pip-mirror:latest .` 或 `docker compose build`。镜像本身不含数据。

下面四个章节按这个顺序展开。

## 模式一：增量更新

外网机日常运行，把"自上次同步以来新增的内容"打成一个 `tar.gz` 拿到内网合并。

外网机：

```bash
./target/release/pip-mirror sync
```

会依次跑两件事：

1. wheel/sdist 同步：按配置里的 `packages` 拉顶层包及其依赖，跳过 `.store.db` 里已记录的文件。
2. Python 解释器同步：从 `python-build-standalone` 取最新 metadata，只下载本地缺失或 sha256 不匹配的版本。

两个阶段独立处理，任一阶段失败会返回退出码 1，但另一阶段仍会跑完。

只要本次有新文件下载，就会在 `incremental/` 目录下产出 `incremental_<UTC>.tar.gz`。包内只含本次新文件 + `manifest.json`（只有 `created_at` 与 `stats`，不含 sha256）。本次 wheel 与 Python 解释器都没有新增则不产文件，日志写 `no changes`，退出码仍为 0。

把单个 tar.gz 拷到内网，合并：

```bash
# 裸跑
./target/release/pip-mirror import-incremental ./incremental_20260504_120000.tar.gz

# 容器内
docker compose exec pip-mirror \
  /pip-mirror import-incremental /repo/packages/incremental_20260504_120000.tar.gz
```

`import-incremental` 会：

1. 校验所有 tar 成员路径都落在 `repository_dir` 内（防 path traversal），然后解包。
2. 对每个解出来的 wheel/sdist 现算 sha256 并写入 `.store.db`，带 `.metadata` 兄弟文件的同样写入 metadata sha256。**sha256 现算，不依赖增量包内的预计算值**。
3. 删除根目录的 `manifest.json`，避免污染仓库。
4. 重建 PEP 503/691 索引，客户端立刻看到新版本（无需重启 server）。

两个开关：

- `--no-reindex`：跳过自动重建索引，适合一次导入多个增量包后统一重建。
- `--strict`：任一文件 sha256 失败会立即 fail-fast 退出 1，**不重建索引**。默认是宽松模式（单文件失败则 WARNING + skip，其它继续）。

## 模式二：全量更新

首次部署、灾后重建、镜像源切换 —— 任何"想要一个完整、自洽的仓库快照"的场景。

外网机：

```bash
./target/release/pip-mirror sync-full
```

行为：

1. 清空 `packages/simple/`、`packages/python-builds/` 与 `.store.db`（`.access_log.db` 不动）。
2. 跑 wheel + Python 解释器同步，等同模式一，但因为目录已清空所以全部都是新下载。
3. 重建索引。
4. 把 `packages/` 整个目录打到仓库根目录的 `mirror.tar.gz`（默认 gzip level 9，可通过 `PIP_MIRROR_TAR_COMPRESSION=none` 关闭压缩；显式排除 `.access_log.db`）。
5. 同目录写 `mirror.sha256`，格式与 `sha256sum` 兼容（`<sha256>  mirror.tar.gz`）。

`sync-full` **不**会产出 `incremental_*.tar.gz`。模式一与模式二产物互斥，不要混用。

把 `mirror.tar.gz` 与 `mirror.sha256` 拷到内网 host，放在 `docker-compose.yml` 同目录：

```bash
sha256sum -c mirror.sha256       # 验签，必须 OK
tar -xzf mirror.tar.gz           # 解出 ./packages/
```

随后走模式三启动服务即可。

`mirror.tar.gz` 还有一个 CI 来源：GitHub Actions 的 E2E workflow 在 rust-rewrite 分支 push 时跑一次全量同步，artifact 里就是这个文件。

## 模式三：服务启动

内网消费端，把仓库通过 HTTP 暴露给客户端。

裸跑：

```bash
./target/release/pip-mirror serve --port 8080
```

或用容器，`docker-compose.yml` 已配好 `network_mode: host` 与 volume 挂载：

```bash
docker compose up -d
```

容器把 `./packages` 挂到 `/repo/packages`，所有数据（simple/、python-builds/、`.store.db`、`.access_log.db`）都持久化在 host。

`network_mode: host` 让容器直接共享 host 网络栈，server 看到的 client IP 就是真实客户端 IP，access-log 的 IP 统计能正常区分内网各客户端。如果改 bridge 模式，前面要架反代设 `X-Forwarded-For`（代码已支持）。

客户端三选一即可使用：

**uv + 环境变量**

```bash
export UV_DEFAULT_INDEX=http://<内网IP>:8080/simple
export UV_PYTHON_DOWNLOADS_JSON_URL=http://<内网IP>:8080/python-builds/index.json
uv python install 3.12
uv pip install requests
```

**pip + 全局配置**

```bash
pip config set global.index-url http://<内网IP>:8080/simple
pip config set global.trusted-host <内网IP>     # 没 HTTPS 时必填
```

**pip + 配置文件**

```
# Linux:   ~/.config/pip/pip.conf
# Windows: %APPDATA%\pip\pip.ini

[global]
index-url = http://192.168.1.100:8080/simple
trusted-host = 192.168.1.100
```

查看访问日志：

```bash
./target/release/pip-mirror access-log -n 30
```

## 模式四：构建 Docker 镜像

部署运维场景，产出可分发的镜像本体（不含数据）。

```bash
# 直接 build
docker build -t pip-mirror:latest .

# 或通过 compose 触发同样的 build
docker compose build
```

`Dockerfile` 是 musl 静态编译 + scratch，镜像体积约 10 MB。镜像里不含 `packages/`，运行时通过 volume 挂载来注入仓库数据。

## 配置

支持两种配置来源，优先级从高到低：

1. `-c` 指定独立 TOML 文件
2. 环境变量 `PIP_MIRROR_PACKAGES`（逗号分隔包名，其余取默认值）

生成示例配置文件：

```bash
./target/release/pip-mirror init -o pip-mirror.toml
```

独立 TOML 文件示例：

```toml
packages = [
    "requests",
    "gradio",
    "markitdown[pptx,docx,xls,xlsx,pdf]",
]
repository_dir = "./packages"
incremental_dir = "./incremental"
pypi_url = "https://pypi.org"
index_url = "https://mirrors.ustc.edu.cn/pypi/simple"
include_source = false
resolve_workers = 8
metadata_workers = 32
download_workers = 8
adjacent_versions_per_side = 2
top_versions_per_package = 5
allow_prerelease = false
linux_max_glibc = "2.39"
server_host = "0.0.0.0"
server_port = 8080
```

核心字段说明：

- `packages`：顶层包列表，字符串数组，支持 `markitdown[pptx,docx,xls,xlsx,pdf]` 这种 extras 写法。
- `repository_dir`：镜像根目录，默认 `./packages`。
- `incremental_dir`：增量包输出目录，默认 `./incremental`。
- `pypi_url`：PyPI JSON API 源，用于取 metadata，默认 `https://pypi.org`。
- `index_url`：Simple Index 源，用于实际下载文件。默认 `https://mirrors.ustc.edu.cn/pypi/simple`（国内可换清华、阿里云）。
- `include_source`：缺平台 wheel 时是否回退 sdist，默认 `false`。
- `resolve_workers`：顶层版本发现和 `(包, 版本, target)` 求解任务的并发上限，默认 `8`。
- `metadata_workers`：PyPI 元数据请求总并发上限，默认 `32`。这是全局请求上限，不是“每个包”的并发。
- `download_workers`：文件下载并发上限，默认 `8`。
- `adjacent_versions_per_side`：每个已解析版本两侧保留的相邻版本数，默认 `2`。
- `top_versions_per_package`：每个包保留的最新版本数，默认 `5`。`0` 表示保留全部版本。
- `allow_prerelease`：是否下载预发行版（rc/alpha/beta/dev），默认 `false`。
- `linux_max_glibc`：Linux 目标接受的最高 glibc 版本，默认 `2.39`。
- `server_host` / `server_port`：`pip-mirror serve` 的监听地址，默认 `127.0.0.1` / `8080`。

命令行覆盖：

```bash
# 临时指定包列表（覆盖配置文件）
./target/release/pip-mirror sync -p requests gradio

# 临时关闭依赖解析
./target/release/pip-mirror sync --no-deps

# 临时指定 host/port
./target/release/pip-mirror serve --host 127.0.0.1 --port 18080
```

## CI

仓库 `.github/workflows/`：

- `rust-ci.yml` —— push/PR 到 `rust-rewrite` 分支时自动跑 `cargo test` + `cargo clippy` + `cargo fmt --check` + 圈复杂度检查。
- `e2e.yml` —— push/PR 到 `rust-rewrite` 分支时跑端到端测试，串完 sync-full + serve + uv pip install + uv python install + sync + import-incremental 全链路。Linux job 产 `mirror.tar.gz`，Windows job 下载后在 Windows runner 上验证客户端安装。

## 仓库布局

```
packages/
├── simple/                          # PEP 503 包索引
│   ├── requests/
│   │   ├── requests-2.32.0-py3-none-any.whl
│   │   ├── requests-2.32.0-py3-none-any.whl.metadata   # PEP 658
│   │   ├── index.html               # PEP 503
│   │   └── index.json               # PEP 691
│   └── ...
├── python-builds/                   # uv 用的解释器
│   ├── index.json                   # uv 读这个，server 会动态把相对 url 改写为绝对
│   └── cpython-*.tar.gz
├── .store.db                        # SQLite，sha256 + metadata_sha256
└── .access_log.db                   # SQLite，访问日志
```

`.access_log.db` 不会被 `sync-full` 打入 `mirror.tar.gz`，内网导入数据时也不会被覆盖，放心保留。
