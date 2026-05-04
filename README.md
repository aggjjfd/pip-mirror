# pip-mirror

轻量级私有 PyPI 镜像，支持增量同步和内网部署。

## 功能

- **PyPI 包镜像**：同步指定包及其依赖，生成 PEP 503 Simple Index
- **Python 解释器镜像**：同步 `python-build-standalone`，支持 `uv python install` 内网使用
- 多平台 wheel 过滤（Windows x86/x64、Linux x64），自动排除 ARM/musl/macOS
- 纯 Python 包自动 fallback 到 sdist
- SQLite 增量跟踪，跳过已下载文件
- 增量 tar.gz 打包，方便离线部署
- 内置 HTTP 服务器，一台服务器同时提供 pip 包和 Python 解释器

## 安装

```bash
git clone https://github.com/aggjjfd/pip-mirror.git
cd pip-mirror
uv sync
```

## 配置

编辑 `pyproject.toml` 中的 `[tool.pip-mirror]` 段：

```toml
[tool.pip-mirror]
packages = [
    "requests",
    "numpy",
    "gradio",
    "markitdown[pptx,docx,xls,xlsx,pdf]",
]
repository_dir = "./packages"
incremental_dir = "./incremental"
pypi_url = "https://pypi.org"
index_url = "https://mirrors.ustc.edu.cn/pypi/simple"
include_source = true
workers = 8
max_versions = 5
```

## 使用

### 同步包

```bash
# 同步配置文件中的包
uv run pip-mirror sync

# 同步指定包
uv run pip-mirror sync --packages requests numpy

# 不同步依赖
uv run pip-mirror sync --no-deps

# 显示 DEBUG 日志
uv run pip-mirror sync -v
```

### 启动 HTTP 服务

```bash
uv run pip-mirror serve --port 8080
```

### 生成示例配置

```bash
uv run pip-mirror init -o pip-mirror.toml
```

### 同步 Python 解释器

```bash
# 同步 uv 使用的 Python 解释器（python-build-standalone）
uv run pip-mirror sync-python

# 指定并发数
uv run pip-mirror sync-python --workers 8
```

支持的版本和平台：
- Python 3.8 ~ 3.14（每个版本的最新 build）
- Windows x86 (32-bit)、Windows x64 (64-bit)
- Linux x64 glibc（含 x86_64 / x86_64_v2 / x86_64_v3 / x86_64_v4 微架构）

## 仓库目录结构

同步完成后：

```
packages/
├── simple/                          # PEP 503 包索引
│   ├── requests/
│   ├── numpy/
│   └── ...
└── python-builds/                   # Python 解释器
    ├── index.json                   # uv 使用的元数据索引
    ├── cpython-3.12.4+20240713-x86_64-pc-windows-msvc-install_only_stripped.tar.gz
    ├── cpython-3.12.4+20240713-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz
    └── ...
```

## 内网 uv 完整配置

在同一台服务器上同时提供 pip 包和 Python 解释器：

```bash
export UV_DEFAULT_INDEX=http://192.168.1.100:8080/simple
export UV_PYTHON_DOWNLOADS_JSON_URL=http://192.168.1.100:8080/python-builds/index.json

# 安装 Python 3.12
uv python install 3.12

# 创建虚拟环境并安装包
uv venv --python 3.12
uv pip install requests numpy
```

如果服务器未配置 HTTPS，需要同时设置：
```bash
export UV_TRUSTED_HOST=192.168.1.100
export PIP_TRUSTED_HOST=192.168.1.100
```

## 内网 pip 配置（Python 包）

### 方式一：命令行临时指定

```bash
pip install --index-url http://<服务器IP>:8080/simple requests
```

### 方式二：pip 全局配置

```bash
pip config set global.index-url http://<服务器IP>:8080/simple
```

或在 `~/.config/pip/pip.conf`（Linux）或 `%APPDATA%\pip\pip.ini`（Windows）中写入：

```ini
[global]
index-url = http://192.168.1.100:8080/simple
trusted-host = 192.168.1.100
```

### 方式三：环境变量

```bash
export PIP_INDEX_URL=http://<服务器IP>:8080/simple
export PIP_TRUSTED_HOST=<服务器IP>
pip install requests
```

### 方式四：项目级配置

在项目目录创建 `pip.conf`：

```ini
[global]
index-url = http://192.168.1.100:8080/simple
trusted-host = 192.168.1.100
```

然后使用：

```bash
pip install --config-file pip.conf requests
```

## Docker 部署

仓库根目录提供 `Dockerfile` 与 `docker-compose.yml`(基于 `python:3.12-slim` 多阶段构建,镜像约 130 MB,只装运行依赖,不打包任何 packages)。

```bash
# 1. 把外网跑出来的 mirror.tar.gz 解压到本地 packages/
tar -xzf mirror.tar.gz

# 2. 起服务(host 网络模式,直接用 host 8080)
docker compose up -d --build

# 3. 验证
curl http://localhost:8080/simple
```

挂载点:`./packages` → `/repo/packages`,内含 `simple/`、`python-builds/`、`.store.db`、`.access_log.db`,容器重启后数据全部保留在 host。

**为什么用 `network_mode: host`**:默认 bridge 网络下 Docker 会做 SNAT,服务里 `client_address[0]` 永远是 docker bridge 网关 IP(如 `172.17.0.1`),`access-log` 统计的所有客户端 IP 都成了同一个值,毫无区分意义。host 模式下容器直接共享 host 网络栈,看到真实客户端 IP。

如果你必须用 bridge 网络(例如 Windows/Mac、要做端口隔离),前面架 nginx/traefik 反向代理,把 `proxy_set_header X-Forwarded-For $remote_addr` 配上,服务端会优先读 `X-Forwarded-For`。

## GitHub Actions 自动同步

项目包含 `.github/workflows/sync.yml`，支持：

- **定时触发**：每周一凌晨 3 点自动同步
- **手动触发**：可指定包列表、版本数、镜像源

在 GitHub 仓库页面的 Actions > Sync PyPI Mirror > Run workflow 中手动执行。

跑完后产物 `mirror.tar.gz`(用 `gzip -9` 高密度压缩) 作为 artifact 上传,内含完整的 `packages/` 目录(simple/ + python-builds/ + .store.db),解压挂上即可使用。

## 增量部署

### 全量首次部署

把 GitHub Actions 跑出的 `mirror.tar.gz` 拷到内网,解压到 `repository_dir`(默认 `./packages`):

```bash
tar -xzf mirror.tar.gz   # 解压后得到 packages/
docker compose up -d
```

### 增量更新(B 方案)

外网每次 `pip-mirror sync` 跑完后,`incremental/` 目录会生成 `incremental_YYYYMMDD_HHMMSS.tar.gz`,内含本次新增的 wheel/sdist、对应的 `.whl.metadata` 文件,以及一份 `manifest.json`(含 sha256 与 metadata_sha256)。

把这一个增量包拷到内网,然后跑 `import-incremental` 命令一次合并:

```bash
# 在内网服务器(裸跑)
pip-mirror import-incremental ./incremental_20260504_120000.tar.gz

# 或容器化部署:
docker compose exec pip-mirror \
    pip-mirror import-incremental /repo/packages/incremental_20260504_120000.tar.gz
```

`import-incremental` 会:
1. 把 tar 解到 `repository_dir`(新 wheel + 新 .metadata 落到 `simple/<pkg>/`)
2. 读 `manifest.json`,把每条记录 `INSERT OR REPLACE` 进 `.store.db` 的 `downloaded_files` + `file_metadata` 表
3. 调用 `generate_index()` 重建 PEP 503 / PEP 691 索引,生成的 `index.html`/`index.json` 自动带上新文件的 `data-sha256`、`data-core-metadata` 与 PEP 658 metadata 链接

完成后客户端立刻能看到新版本,**不必重启服务**(server 是静态文件 + 内容协商,新生成的索引文件下次请求即生效)。如要跳过自动重建索引(例如批量导入多个增量包),加 `--no-reindex`,最后再手动跑一次。

### Python 解释器离线部署

`packages/python-builds/` 目录已经在 `mirror.tar.gz` 里,首次部署解压后即可。增量同步 `python-build-standalone` 在当前阶段不通过 `import-incremental`,直接用 `rsync` 覆盖目录:

```bash
rsync -av packages/python-builds/ 内网服务器:/path/to/packages/python-builds/
```

### 启动服务

```bash
uv run pip-mirror serve --port 8080
```

服务器会自动提供：
- `http://server:8080/simple` — pip 包索引
- `http://server:8080/python-builds/index.json` — Python 解释器元数据
