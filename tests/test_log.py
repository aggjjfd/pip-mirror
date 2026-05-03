"""F2 回归: setup_logging 必须让 DEBUG 真正打印,而非被 handler 上的 filter 静默吞掉.

验证策略:不仅检查 logger 自身的 effective level,还要:
1. 静态地断言 colorlog handler 上不带任何 logging.Filter (回滚 F2 即破)。
2. 行为地写一条 DEBUG,断言 colorlog handler 真的把它写到了流上。
"""

from __future__ import annotations

import io
import logging

from pip_mirror.log import setup_logging


def _pip_mirror_handler() -> logging.Handler:
    handlers = logging.getLogger("pip-mirror").handlers
    assert len(handlers) == 1, f"setup_logging should attach exactly one handler, got {handlers}"
    return handlers[0]


def test_setup_logging_attaches_no_blocking_filter() -> None:
    """colorlog handler 上不应挂任何 filter — 否则 -v 下的 DEBUG 会被静默吞掉(F2 原始症状)."""
    setup_logging(logging.DEBUG)
    handler = _pip_mirror_handler()
    assert handler.filters == [], (
        f"handler picked up unexpected filters {handler.filters} — "
        f"this would silently drop DEBUG records, the F2 regression"
    )


def test_debug_record_reaches_handler_stream_when_level_debug() -> None:
    """在 DEBUG level 下,debug() 必须真的把文本写到 colorlog handler 的输出流."""
    setup_logging(logging.DEBUG)
    handler = _pip_mirror_handler()

    buffer = io.StringIO()
    original_stream = handler.stream
    handler.stream = buffer
    try:
        logging.getLogger("pip-mirror").debug("regression-canary-debug")
        handler.flush()
    finally:
        handler.stream = original_stream

    text = buffer.getvalue()
    assert "regression-canary-debug" in text, (
        f"DEBUG record did not reach the colorlog handler stream — F2 regression. Got: {text!r}"
    )
    assert "DEBUG" in text


def test_debug_record_blocked_when_level_info() -> None:
    """在 INFO level 下,debug() 不应到达 handler stream(verifies level wiring works at all)."""
    setup_logging(logging.INFO)
    handler = _pip_mirror_handler()

    buffer = io.StringIO()
    original_stream = handler.stream
    handler.stream = buffer
    try:
        logger = logging.getLogger("pip-mirror")
        logger.debug("should-not-appear")
        logger.info("info-marker")
        handler.flush()
    finally:
        handler.stream = original_stream

    text = buffer.getvalue()
    assert "should-not-appear" not in text
    assert "info-marker" in text
