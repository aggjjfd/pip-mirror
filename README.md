# pip-mirror

轻量级私有 PyPI 镜像。一台服务器同时供 `pip install` 与 `uv python install` 用,适合内网离线部署。

- 同步 PyPI 包 + 自动解析依赖
- 同步 `python-build-standalone` 解释器(uv 兼容)
- PEP 503 / PEP 658 / PEP 691 索引(含 sha256 与独立 metadata)
- SQLite 增量记录,跳过已下载文件
- 增量包(`incremental_*.tar.gz`)与全量包(`mirror.tar.gz`)两种部署形态
- 内置 HTTP server,支持反代 `X-Forwarded-For`,记录访问日志

## 安装

```bash
git clone https://github.com/aggjjfd/pip-mirror.git
cd pip-mirror
uv sync
```

## 5 分钟快速上手

外网机:

```bash
uv run pip-mirror sync           # 同步 pyproject.toml 里的包及全部依赖
uv run pip-mirror sync-python    # 顺手把 Python 解释器拉一份
```

把 `packages/` 整个拷到内网,然后:

```bash
docker compose up -d --build
```

客户端:

```bash
export UV_DEFAULT_INDEX=http://<内网IP>:8080/simple
export UV_PYTHON_DOWNLOADS_JSON_URL=http://<内网IP>:8080/python-builds/index.json
uv python install 3.12
uv pip install requests
```

## 配置

打开 `pyproject.toml` 修改 `[tool.pip-mirror]` 段,核心字段:

```toml
[tool.pip-mirror]
packages = ["requests", "numpy", "markitdown[pptx,docx,xls,xlsx,pdf]"]  # 顶层包
repository_dir = "./packages"     # 镜像根目录
incremental_dir = "./incremental" # 增量包输出目录
pypi_url   = "https://pypi.org"                             # JSON API 源(取 metadata)
index_url  = "https://mirrors.ustc.edu.cn/pypi/simple"      # 下载源
include_source = true     # 缺平台时回退 sdist
workers       = 8          # 并发
max_versions  = 5          # 每个包保留最新 N 个版本
server_host   = "0.0.0.0"
server_port   = 8080
```

也支持 TOML 文件 `pip-mirror.toml`(与 `pyproject.toml [tool.pip-mirror]` 同 schema),用 `-c` 指定。

## 命令清单

| 命令 | 用途 |
|---|---|
| `pip-mirror init -o pip-mirror.toml` | 生成示例配置 |
| `pip-mirror sync` | 同步 PyPI 包(含依赖),写入 `packages/simple/` 与 `.store.db`,产出 `incremental/incremental_*.tar.gz` |
| `pip-mirror sync --no-pack` | 同上,但不打增量包(CI 出全量包时用) |
| `pip-mirror sync-python --workers 8` | 同步 `python-build-standalone`,写入 `packages/python-builds/index.json` |
| `pip-mirror serve --port 8080` | 启动 HTTP 服务,内置 PEP 503/691 内容协商与访问日志 |
| `pip-mirror import-incremental incr.tar.gz` | 内网合并增量包,自动重建索引(`--no-reindex` 跳过) |
| `pip-mirror access-log -n 30` | 查看访问日志统计 |
| `-v / --verbose` | 全局打开 DEBUG 日志 |

## Docker 部署

仓库根目录提供 `Dockerfile`(multi-stage,~130 MB)与 `docker-compose.yml`:

```bash
tar -xzf mirror.tar.gz       # 解出 packages/
docker compose up -d --build
```

- **挂载**:`./packages → /repo/packages`,所有数据(simple/、python-builds/、`.store.db`、`.access_log.db`)持久化在 host
- **`network_mode: host`**:容器直接共享 host 网络栈,server 看到的 `client_address` 即真实客户端 IP,access-log IP 统计有效。如果改 bridge 模式,前面要架反代设 `X-Forwarded-For`(代码已支持)

## 增量更新

外网每次 `sync` 跑完,会在 `incremental/` 生成 `incremental_YYYYMMDD_HHMMSS.tar.gz`,内含本次新增的 wheel/sdist、对应的 `.whl.metadata`,以及 `manifest.json`(含 sha256 + metadata_sha256)。

把这一个文件拷到内网,跑一次:

```bash
# 裸跑
pip-mirror import-incremental ./incremental_20260504_120000.tar.gz

# 容器内
docker compose exec pip-mirror \
  pip-mirror import-incremental /repo/packages/incremental_20260504_120000.tar.gz
```

会自动:
1. 解包到 `repository_dir`(新文件落到 `simple/<pkg>/`,带 PEP 658 metadata)
2. 把 manifest 的 sha256 + metadata_sha256 `INSERT OR REPLACE` 进 `.store.db`
3. 跑 `generate_index()` 重建 PEP 503/691 索引,客户端立刻看到新版本(无需重启 server)

加 `--no-reindex` 可跳过自动重建,适合批量导入多个增量包后统一重建。

## GitHub Actions

`.github/workflows/`:

- **`ci.yml`** — push/PR 自动跑 ruff + pytest(matrix Python 3.8/3.11/3.12/3.13)
- **`sync.yml`** — 周一凌晨 3 点定时全量同步;手动可指定包列表/版本数/镜像源。产物 `mirror.tar.gz` 作为 artifact 上传(gzip -9)
- **`verify-install.yml`** — 手动触发,做一次全量 sync + 多个独立 venv 各装一两个不同顶层包,验证 mirror 自身完整性
- **`test-install.yml`** — 老的"装单个包"验证,跨 Linux/Windows × Python 3.8/3.11/3.12/3.13

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
│   ├── index.json                   # uv 读这个,server 会动态把相对 url 改写为绝对
│   └── cpython-*.tar.gz
├── .store.db                        # SQLite,sha256 + metadata_sha256
└── .access_log.db                   # SQLite,访问日志
```

## 客户端配置(其它形态)

如果不用环境变量:

```bash
pip config set global.index-url http://<内网IP>:8080/simple
pip config set global.trusted-host <内网IP>     # 没 HTTPS 时
```

或写到 `~/.config/pip/pip.conf`(Linux)/ `%APPDATA%\pip\pip.ini`(Windows):

```ini
[global]
index-url = http://192.168.1.100:8080/simple
trusted-host = 192.168.1.100
```
