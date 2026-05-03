# pip-mirror

轻量级私有 PyPI 镜像，支持增量同步和内网部署。

## 功能

- 从 PyPI/镜像站同步指定包及其依赖
- 保留多平台 wheel（Windows x86/x64、Linux x64），自动过滤 ARM/musl/macOS
- 纯 Python 包自动 fallback 到 sdist，非纯 Python 包缺失平台时告警
- SQLite 增量跟踪，跳过已下载文件
- 生成 PEP 503 Simple Index 目录结构
- 内置 HTTP 服务器，内网直接作为 pip 源使用
- 增量 tar.gz 打包，方便离线部署

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

## 内网 pip 配置

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

## GitHub Actions 自动同步

项目包含 `.github/workflows/sync.yml`，支持：

- **定时触发**：每周一凌晨 3 点自动同步
- **手动触发**：可指定包列表、版本数、镜像源

在 GitHub 仓库页面的 Actions > Sync PyPI Mirror > Run workflow 中手动执行。

## 增量部署

同步完成后，`incremental/` 目录会生成 `incremental_YYYYMMDD_HHMMSS.tar.gz`，包含本次新增文件和 `manifest.json`。

在内网服务器解压到仓库目录：

```bash
tar -xzf incremental_20260503_120000.tar.gz -C /path/to/packages
```

然后重新生成索引：

```bash
uv run pip-mirror serve --port 8080
# 或只生成索引不启动服务（当前版本 serve 会自动生成）
```
