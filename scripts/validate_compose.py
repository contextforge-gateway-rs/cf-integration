#!/usr/bin/env python3
"""Fail closed when the rendered integration Compose contract drifts."""

from __future__ import annotations

import json
import os
import sys

LEGACY_PREFIXES = (
    "ghcr.io/ibm/fast-time-server:",
    "ghcr.io/ibm/fast-time-server@",
    "mcpgateway/fast-test-server:",
    "mcpgateway/fast-test-server@",
)


def command_text(value) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return " ".join(map(str, value))
    return ""


def violations(config: dict, expected_image: str) -> list[str]:
    services = config.get("services")
    if not isinstance(services, dict):
        return ["rendered Compose config has no services object"]
    issues = []
    fast_time = services.get("fast_time_server")
    if not isinstance(fast_time, dict):
        issues.append("fast_time_server is missing from the integration Compose config")
    elif fast_time.get("image") != expected_image:
        issues.append(
            f"fast_time_server image is {fast_time.get('image')!r}; expected {expected_image!r}"
        )
    for name in ("fast_test_server", "register_fast_test"):
        service = services.get(name)
        if isinstance(service, dict) and not service.get("profiles"):
            issues.append(f"{name} is active without an explicit profile")
    for name, service in sorted(services.items()):
        image = service.get("image", "") if isinstance(service, dict) else ""
        if any(str(image).startswith(prefix) for prefix in LEGACY_PREFIXES):
            issues.append(f"{name} uses legacy fast-test/time image {image!r}")
    registration = command_text(services.get("register_fast_time", {}).get("command"))
    if "http://fast_time_server:9080/health" not in registration:
        issues.append("register_fast_time does not wait for Fast Time health on port 9080")
    if "http://fast_time_server:9080/mcp" not in registration:
        issues.append("register_fast_time does not register the streamable HTTP /mcp endpoint")
    return issues


def main() -> None:
    config = json.load(sys.stdin)
    expected = os.environ.get(
        "CF_FAST_TIME_EXPECTED_IMAGE",
        os.environ.get("FAST_TIME_IMAGE", "ghcr.io/ibm/cfex-mcp-fast-time-server:latest"),
    )
    issues = violations(config, expected)
    if issues:
        print("Compose contract failed:", file=sys.stderr)
        for issue in issues:
            print(f"  - {issue}", file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
