"""日志配置：彩色控制台输出."""

from __future__ import annotations

import logging
import sys

import colorlog


LOG_FORMAT = "%(log_color)s%(levelname)-8s%(reset)s %(message)s"
LOG_DATE_FORMAT = "%H:%M:%S"


class InfoFilter(logging.Filter):
    """只通过 INFO 及以上级别的日志."""

    def filter(self, record: logging.LogRecord) -> bool:
        return record.levelno >= logging.INFO


def setup_logging(level: int = logging.INFO) -> None:
    """配置彩色控制台日志.

    Args:
        level: 日志级别，默认 INFO
    """
    handler = colorlog.StreamHandler(sys.stdout)
    handler.setFormatter(
        colorlog.ColoredFormatter(
            LOG_FORMAT,
            datefmt=LOG_DATE_FORMAT,
            log_colors={
                "DEBUG": "cyan",
                "INFO": "green",
                "WARNING": "yellow",
                "ERROR": "red",
                "CRITICAL": "red,bg_white",
            },
        ),
    )
    handler.addFilter(InfoFilter())

    root = logging.getLogger("pip-mirror")
    root.setLevel(level)
    root.handlers.clear()
    root.addHandler(handler)
