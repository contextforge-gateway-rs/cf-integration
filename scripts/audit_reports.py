#!/usr/bin/env python3
"""Remove and report generated artifacts containing a bearer credential."""

from __future__ import annotations

import os
import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: audit_reports.py REPORT_DIRECTORY")
    token = os.environ.get("AUDIT_TOKEN", "").encode()
    if not token:
        raise SystemExit("AUDIT_TOKEN is required")
    root = Path(sys.argv[1])
    tainted = []
    for candidate in root.rglob("*"):
        if candidate.is_symlink() or not candidate.is_file():
            continue
        if token in candidate.read_bytes():
            tainted.append(candidate)
    for candidate in tainted:
        candidate.unlink()
    if tainted:
        raise SystemExit(
            f"removed {len(tainted)} report artifact(s) containing a bearer credential"
        )


if __name__ == "__main__":
    main()
