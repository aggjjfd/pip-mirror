# Changelog

## [1.0.0] - 2026-05-18

### Features

- 内网私有 PyPI 镜像服务（PEP 503 / PEP 691）
- 增量/全量同步 PyPI 包，支持指定目标环境（Python 版本 × OS × 架构）
- 依赖解析优化：求解缓存、前置剪枝与解析缓存
- 内嵌 uv 安装器（内网一键安装 uv，无需外网）
- Web 首页：包列表搜索、目标环境展示、pip/uv 使用方式一键复制
- 支持 `--dry-run` 参数，仅解析依赖不下载
- 支持 musl 静态编译，单二进制可执行
- python-builds 索引生成，uv python install 内网可用

### Infrastructure

- E2E 集成测试（四种模式覆盖 Linux / Windows）
- GitHub Actions Release CI，自动编译 Linux + Windows 产物
