"""Shared MCP streamable-HTTP helpers for cf-probe.py and the harness locustfile.

The compose overlay mounts this file next to the locustfile; cf-probe.py
imports it from the same directory on the host.
"""
from __future__ import annotations

import json
import uuid

PROTOCOL_VERSION = "2025-06-18"
ACCEPT = "application/json, text/event-stream"


def jsonrpc(method: str, params: dict | None = None) -> dict:
    payload = {"jsonrpc": "2.0", "id": str(uuid.uuid4()), "method": method}
    if params is not None:
        payload["params"] = params
    return payload


def parse_mcp_body(text: str, content_type: str):
    """Return the JSON-RPC message from a JSON or SSE response body."""
    if "text/event-stream" in content_type:
        message = None
        for line in text.splitlines():
            if line.startswith("data:"):
                try:
                    message = json.loads(line[len("data:"):].strip())
                except ValueError:
                    pass
        return message
    return json.loads(text) if text else None


def tool_call_args(tool_name: str) -> dict | None:
    """Return call arguments for tools this harness knows how to invoke."""
    if tool_name.endswith("echo"):
        return {"message": "cf-integration"}
    if tool_name.endswith("get_system_time"):
        return {"timezone": "UTC"}
    return None
