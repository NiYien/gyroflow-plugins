#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Retry codesign when Apple's timestamp response is missing."""
from __future__ import annotations

import os
import subprocess
import sys
import time


RETRY_DELAYS = (5, 15, 30)
MISSING_TIMESTAMP = "A timestamp was expected but was not found"


def codesign_with_retry(arguments: list[str]) -> int:
    # Keep all signing requirements and retry only the known timestamp failure.
    command = ["codesign", *arguments]
    environment = {**os.environ, "LC_ALL": "C"}
    for attempt in range(len(RETRY_DELAYS) + 1):
        result = subprocess.run(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            env=environment,
            check=False,
        )
        print(result.stdout, end="", file=sys.stderr, flush=True)
        if result.returncode == 0:
            return 0
        if MISSING_TIMESTAMP not in result.stdout or attempt == len(RETRY_DELAYS):
            return result.returncode
        delay = RETRY_DELAYS[attempt]
        print(
            f"codesign did not receive a timestamp; retrying in {delay}s "
            f"(attempt {attempt + 2}/{len(RETRY_DELAYS) + 1}).",
            file=sys.stderr,
            flush=True,
        )
        time.sleep(delay)
    raise AssertionError("Unreachable retry state")


if __name__ == "__main__":
    raise SystemExit(codesign_with_retry(sys.argv[1:]))
