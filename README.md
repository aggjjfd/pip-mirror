# pip-mirror

轻量级私有 PyPI 镜像。一台服务器同时供 `pip install` 与 `uv python install` 使用,适合内网离线部署。

特性:

- 同步 PyPI 包并自动解析依赖
- 同步 `python-build-standalone` 解释器(uv 兼容)
- PEP 503 / PEP 658 / PEP 691 索引(含 sha256 与独立 metadata)
- SQLite 增量记录,跳过已下载文件
- 内置 HTTP server,支持反代 `X-Forwarded-For`,记录访问日志

## 安装

**Rust 版**（推荐）：

```bash
git clone https://github.com/aggjjfd/pip-mirror.git
cd pip-mirror
cargo build --release
./target/release/pip-mirror --help
```

**Python 版**（main 分支，功能完整但较慢）：

```bash
git clone https://github.com/aggjjfd/pip-mirror.git
cd pip-mirror
uv sync
```

## 四种使用模式

对外只有四种使用模式,每种模式都有清楚的单一命令、单一产物。请先确认自己处在哪种模式,再去看对应的章节。

- **模式一:增量更新** —— `pip-mirror sync`,产出 `incremental/incremental_*.tar.gz`。日常用,只下载 wheel 与 Python 解释器的新增部分。
- **模式二:全量更新** —— `pip-mirror sync-full`,产出仓库根目录的 `mirror.tar.gz` + `mirror.sha256`。首次部署或灾后重建用,会先清空仓库再重拉。
- **模式三:服务启动** —— `pip-mirror serve` 或 `docker compose up -d`。内网消费端用。
- **模式四:构建 Docker 镜像** —— `docker build -t pip-mirror:latest .` 或 `docker compose build`。镜像本身不含数据。

下面四个章节按这个顺序展开。

## 模式一:增量更新

外网机日常运行,把"自上次同步以来新增的内容"打成一个 `tar.gz` 拿到内网合并。

外网机:

```bash
uv run pip-mirror sync
```

会依次跑两件事:

1. wheel/sdist 同步:按 `[tool.pip-mirror].packages` 配置拉顶层包及其依赖,跳过 `.store.db` 里已记录的文件。
2. Python 解释器同步:从 `python-build-standalone` 取最新 metadata,只下载本地缺失或 sha256 不匹配的版本。

两个阶段独立 try/except,任一阶段失败 → 退出码 1,但另一阶段仍会跑完。

只要本次有新文件下载,就会在 `incremental/` 目录下产出 `incremental_<UTC>.tar.gz`。包内只含本次新文件 + `manifest.json`(只有 `created_at` 与 `stats`,不含 sha256)。本次 wheel 与 Python 解释器都没有新增 → 不产文件,日志写 `no changes`,退出码仍为 0。

把单个 tar.gz 拷到内网,合并:

```bash
# 裸跑
pip-mirror import-incremental ./incremental_20260504_120000.tar.gz

# 容器内
docker compose exec pip-mirror \
  pip-mirror import-incremental /repo/packages/incremental_20260504_120000.tar.gz
```

`import-incremental` 会:

1. 校验所有 tar 成员路径都落在 `repository_dir` 内(防 path traversal),然后解包(Python 3.12+ 用 `filter="data"`)。
2. 对每个解出来的 wheel/sdist 现算 sha256 并写入 `.store.db`,带 `.metadata` 兄弟文件的同样写入 metadata sha256。**sha256 现算,不依赖增量包内的预计算值**。
3. 删除根目录的 `manifest.json`,避免污染仓库。
4. 调 `generate_index()` 重建 PEP 503/691 索引,客户端立刻看到新版本(无需重启 server)。

两个开关:

- `--no-reindex`:跳过自动重建索引,适合一次导入多个增量包后统一重建。
- `--strict`:任一文件 sha256 失败 → 立即 fail-fast 退出 1,**不重建索引**。默认是宽松模式(单文件失败 → WARNING + skip,其它继续)。

## 模式二:全量更新

首次部署、灾后重建、镜像源切换 ── 任何"想要一个完整、自洽的仓库快照"的场景。

外网机:

```bash
uv run pip-mirror sync-full
```

行为:

1. 清空 `packages/simple/`、`packages/python-builds/` 与 `.store.db`(`.access_log.db` 不动)。
2. 跑 wheel + Python 解释器同步,等同模式一,但因为目录已清空所以全部都是新下载。
3. `generate_index()` 重建索引。
4. 把 `packages/` 整个目录打到仓库根目录的 `mirror.tar.gz`(`gzip -9`,显式排除 `.access_log.db`)。
5. 同目录写 `mirror.sha256`,格式与 `sha256sum` 兼容(`<sha256>  mirror.tar.gz`)。

`sync-full` **不**会产出 `incremental_*.tar.gz`。模式一与模式二产物互斥,不要混用。

把 `mirror.tar.gz` 与 `mirror.sha256` 拷到内网 host,放在 `docker-compose.yml` 同目录:

```bash
sha256sum -c mirror.sha256       # 验签,必须 OK
tar -xzf mirror.tar.gz           # 解出 ./packages/
```

随后走模式三启动服务即可。

`mirror.tar.gz` 还有一个 CI 来源:GitHub Actions 的 `sync.yml` workflow 周一定时跑一次 `sync-full`,artifact 里就是这个文件。详见下方 CI 章节。

## 模式三:服务启动

内网消费端,把仓库通过 HTTP 暴露给客户端。

裸跑:

```bash
uv run pip-mirror serve --port 8080
```

或用容器,`docker-compose.yml` 已配好 `network_mode: host` 与 volume 挂载:

```bash
docker compose up -d
```

容器把 `./packages` 挂到 `/repo/packages`,所有数据(simple/、python-builds/、`.store.db`、`.access_log.db`)都持久化在 host。

`network_mode: host` 让容器直接共享 host 网络栈,server 看到的 `client_address[0]` 就是真实客户端 IP,access-log 的 IP 统计能正常区分内网各客户端。如果改 bridge 模式,前面要架反代设 `X-Forwarded-For`(代码已支持)。

客户端三选一即可使用:

```bash
# 1) uv,环境变量
export UV_DEFAULT_INDEX=http://<内网IP>:8080/simple
export UV_PYTHON_DOWNLOADS_JSON_URL=http://<内网IP>:8080/python-builds/index.json
uv python install 3.12
uv pip install requests

# 2) pip,全局配置
pip config set global.index-url http://<内网IP>:8080/simple
pip config set global.trusted-host <内网IP>     # 没 HTTPS 时必填

# 3) pip,配置文件
# Linux:   ~/.config/pip/pip.conf
# Windows: %APPDATA%\pip\pip.ini
[global]
index-url = http://192.168.1.100:8080/simple
trusted-host = 192.168.1.100
```

查看访问日志:

```bash
pip-mirror access-log -n 30
```

## 模式四:构建 Docker 镜像

部署运维场景,产出可分发的镜像本体(不含数据)。

```bash
# 直接 build
docker build -t pip-mirror:latest .

# 或通过 compose 触发同样的 build
docker compose build
```

`Dockerfile` 是 musl 静态编译 + scratch,镜像体积约 10 MB。镜像里不含 `packages/`,运行时通过 volume 挂载来注入仓库数据。

## 配置

打开 `pyproject.toml` 修改 `[tool.pip-mirror]` 段,核心字段:

- `packages`:顶层包列表,字符串数组,支持 `markitdown[pptx,docx,xls,xlsx,pdf]` 这种 extras 写法。
- `repository_dir`:镜像根目录,默认 `./packages`。
- `incremental_dir`:增量包输出目录,默认 `./incremental`。
- `pypi_url`:PyPI JSON API 源,用于取 metadata,默认 `https://pypi.org`。
- `index_url`:Simple Index 源,用于实际下载文件。默认 `https://mirrors.ustc.edu.cn/pypi/simple`(国内可换清华、阿里云)。
- `include_source`:缺平台 wheel 时是否回退 sdist,默认 `true`。
- `workers`:并发下载线程数,默认 `4`。
- `max_versions`:每个包保留的最新版本数,默认 `5`。
- `allow_prerelease`:是否下载预发行版(rc/alpha/beta/dev),默认 `false`。
- `server_host` / `server_port`:`pip-mirror serve` 的监听地址,默认 `0.0.0.0` / `8080`。

也支持独立 TOML 文件 `pip-mirror.toml`(与 `pyproject.toml [tool.pip-mirror]` 同 schema),用 `-c` 指定:

```bash
pip-mirror init -o pip-mirror.toml      # 生成示例
pip-mirror sync -c pip-mirror.toml      # 用独立文件
```

## CI 同步

仓库 `.github/workflows/`:

- **`sync.yml`** —— 周一凌晨 3 点定时跑 `pip-mirror sync-full`,产物 `mirror.tar.gz` + `mirror.sha256` + `report.txt` + `sync.log` 上传成 artifact。手动触发(`workflow_dispatch`)可临时改写 `pyproject.toml` 里的 `max_versions` / `index_url`,或通过 `--packages` / `--no-deps` 透传给 `sync-full`。**这是 `mirror.tar.gz` 的另一个获取通道,本质和模式二在本机跑一次没有区别。**
- **`ci.yml`** —— push/PR 自动跑 ruff + pytest(matrix Python 3.8/3.11/3.12/3.13)。
- **`verify-install.yml`** —— 手动触发,跑一次全量 sync + 多个独立 venv 各装一两个不同顶层包,验证 mirror 自身完整性。
- **`test-install.yml`** —— 老的"装单个包"验证,跨 Linux/Windows × Python 3.8/3.11/3.12/3.13。

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

`.access_log.db` 不会被 `sync-full` 打入 `mirror.tar.gz`,内网导入数据时也不会被覆盖,放心保留。
