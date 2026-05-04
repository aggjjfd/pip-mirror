# syntax=docker/dockerfile:1

# ---- builder ----
FROM python:3.12-slim AS builder

RUN pip install --no-cache-dir uv

WORKDIR /app
COPY pyproject.toml ./
COPY src/ ./src/

# 装到 system site-packages,不要 venv,方便后续 stage 拷贝
RUN uv pip install --system --no-cache .

# ---- runtime ----
FROM python:3.12-slim

# 仅搬运已装好的依赖与可执行入口
COPY --from=builder /usr/local/lib/python3.12/site-packages /usr/local/lib/python3.12/site-packages
COPY --from=builder /usr/local/bin/pip-mirror /usr/local/bin/pip-mirror

# 仓库目录由外部挂载(packages/ + 自动生成的 .access_log.db / .store.db / simple/ python-builds/)
WORKDIR /repo
VOLUME ["/repo/packages"]

# host 网络模式下 EXPOSE 仅作文档
EXPOSE 8080

ENTRYPOINT ["pip-mirror"]
CMD ["serve", "--host", "0.0.0.0", "--port", "8080"]
