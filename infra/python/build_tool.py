#!/usr/bin/env python3
"""Stable profiler entrypoint, symlinked under each traced tool's name."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from build_profile import wrapper

if __name__ == '__main__':
    sys.argv.insert(1, sys.argv[0])
    sys.exit(wrapper())
