"""验证 _session.make_session() 在 http/https 上都挂了带重试的 HTTPAdapter."""

from __future__ import annotations

from urllib3.util.retry import Retry

from pip_mirror._session import make_session


def test_make_session_mounts_adapter_for_http_and_https() -> None:
    session = make_session()
    try:
        for prefix in ("http://", "https://"):
            adapter = session.get_adapter(f"{prefix}example.com")
            assert adapter is not None, f"no adapter for {prefix}"
            retry = adapter.max_retries
            assert isinstance(retry, Retry), f"adapter at {prefix} not configured with Retry"
            assert retry.total == 5
            assert retry.connect == 5
            assert retry.read == 5
            assert retry.status == 5
            assert retry.backoff_factor == 0.5
            assert 503 in retry.status_forcelist
            assert 504 in retry.status_forcelist
    finally:
        session.close()


def test_make_session_respects_custom_retries() -> None:
    session = make_session(retries=2, backoff=1.5)
    try:
        retry = session.get_adapter("https://example.com").max_retries
        assert retry.total == 2
        assert retry.backoff_factor == 1.5
    finally:
        session.close()
