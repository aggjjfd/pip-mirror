"""允许通过 python -m pip_mirror 运行."""

import sys

from .cli import main

if __name__ == "__main__":
    sys.exit(main())
