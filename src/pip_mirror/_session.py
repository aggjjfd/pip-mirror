"""带自动重试的 requests Session 工厂.

对 connection error / 408 / 429 / 5xx 自动重试,使用指数 backoff。
所有需要拉外网的入口(downloader, dependency_resolver, python_downloader)统一调用 make_session()。
"""

from __future__ import annotations

import requests
from requests.adapters import HTTPAdapter
from urllib3.util.retry import Retry


def make_session(retries: int = 5, backoff: float = 0.5) -> requests.Session:
    """构造带自动重试的 requests.Session.

    Args:
        retries: 各阶段最大重试次数(connect/read/status 各自计数)
        backoff: 指数 backoff 因子,第 N 次重试等待 backoff * (2 ** (N-1)) 秒

    Returns:
        已挂载 HTTP/HTTPS HTTPAdapter 的 Session
    """
    retry = Retry(
        total=retries,
        connect=retries,
        read=retries,
        status=retries,
        backoff_factor=backoff,
        status_forcelist=(408, 429, 500, 502, 503, 504),
        allowed_methods=frozenset({"GET", "HEAD"}),
        raise_on_status=False,
    )
    adapter = HTTPAdapter(max_retries=retry)
    session = requests.Session()
    session.mount("http://", adapter)
    session.mount("https://", adapter)
    return session
