#!/usr/bin/env python3
"""Probe the public nginx -> cf-dataplane MCP route.

Runs the report's pass signal as one command:
  1. initialize without Authorization  -> expect 401
  2. initialize with bearer token      -> expect 200 + Mcp-Session-Id
  3. tools/list with the session       -> expect tools
  4. tools/call on an echo/system-time tool through the same session

Exits non-zero on the first failure. Stdlib only.
"""
from __future__ import annotations

import importlib.util
import json
import os
import sys
import urllib.error
import urllib.request
import uuid
from pathlib import Path

_spec = importlib.util.spec_from_file_location("cf_jwt", Path(__file__).with_name("cf-jwt.py"))
cf_jwt = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cf_jwt)

BASE_URL = os.environ.get("MCP_CLI_BASE_URL", "http://127.0.0.1:8080")
SERVER_ID = (
    os.environ.get("MCP_SERVER_ID")
    or os.environ.get("MCP_VIRTUAL_SERVER_ID")
    or os.environ.get("CF_FAST_TIME_SERVER_ID", "9779b6698cbd4b4995ee04a4fab38737")
)
JWT_SECRET_KEY = os.environ.get("JWT_SECRET_KEY", "my-test-key-but-now-longer-than-32-bytes")
ADMIN_EMAIL = os.environ.get("PLATFORM_ADMIN_EMAIL", "admin@example.com")
URL = f"{BASE_URL}/servers/{SERVER_ID}/mcp"
PROTOCOL_VERSION = "2025-06-18"


def fail(step: str, detail: str) -> None:
    print(f"{step}=FAIL {detail}")
    sys.exit(1)


def parse_mcp_body(body: str, content_type: str):
    """Return the JSON-RPC message from a JSON or SSE response body."""
    if "text/event-stream" in content_type:
        message = None
        for line in body.splitlines():
            if line.startswith("data:"):
                try:
                    message = json.loads(line[len("data:"):].strip())
                except ValueError:
                    pass
        return message
    return json.loads(body) if body else None


def mcp_post(payload: dict, token: str | None, session_id: str | None = None):
    """POST a JSON-RPC payload. Returns (status, headers, message)."""
    req = urllib.request.Request(URL, method="POST", data=json.dumps(payload).encode("utf-8"))
    req.add_header("Content-Type", "application/json")
    req.add_header("Accept", "application/json, text/event-stream")
    req.add_header("Mcp-Protocol-Version", PROTOCOL_VERSION)
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    if session_id:
        req.add_header("Mcp-Session-Id", session_id)
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            body = response.read().decode("utf-8")
            message = parse_mcp_body(body, response.headers.get("Content-Type", ""))
            return response.status, response.headers, message
    except urllib.error.HTTPError as exc:
        return exc.code, exc.headers, None


def jsonrpc(method: str, params: dict) -> dict:
    return {"jsonrpc": "2.0", "id": str(uuid.uuid4()), "method": method, "params": params}


def result_of(step: str, status: int, message) -> dict:
    if status != 200:
        fail(step, f"status={status}")
    if not isinstance(message, dict) or "error" in message:
        fail(step, f"unexpected response: {message}")
    return message.get("result") or {}


def main() -> None:
    print(f"probe url: {URL}")
    init_payload = jsonrpc(
        "initialize",
        {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "cf-probe", "version": "1.0"},
        },
    )

    status, _, _ = mcp_post(init_payload, token=None)
    if status != 401:
        fail("auth_negative", f"expected 401 without Authorization, got {status}")
    print(f"auth_negative=PASS status={status}")

    token = os.environ.get("MCPGATEWAY_BEARER_TOKEN") or cf_jwt.make_token(
        JWT_SECRET_KEY,
        ADMIN_EMAIL,
        scopes={
            "server_id": None,
            "permissions": ["servers.use", "tools.read", "tools.call"],
            "ip_restrictions": [],
            "time_restrictions": None,
        },
    )
    status, headers, message = mcp_post(init_payload, token)
    result_of("initialize", status, message)
    session_id = headers.get("Mcp-Session-Id")
    if not session_id:
        fail("initialize", "no Mcp-Session-Id header in response")
    print(f"initialize=PASS status={status} session={session_id}")

    status, _, message = mcp_post(jsonrpc("tools/list", {}), token, session_id)
    tools = result_of("tools_list", status, message).get("tools", [])
    if not tools:
        fail("tools_list", "no tools returned")
    print(f"tools_list=PASS count={len(tools)}")
    for tool in tools:
        print(f"tool={tool.get('name')}")

    tool_names = [tool.get("name", "") for tool in tools]
    call_name, call_args = None, {}
    for name in tool_names:
        if name.endswith("echo"):
            call_name, call_args = name, {"message": "cf-probe"}
            break
        if name.endswith("get_system_time"):
            call_name, call_args = name, {"timezone": "UTC"}
    if call_name is None:
        print("tool_call=SKIP no echo/get_system_time tool available")
        return

    status, _, message = mcp_post(jsonrpc("tools/call", {"name": call_name, "arguments": call_args}), token, session_id)
    result = result_of("tool_call", status, message)
    if result.get("isError"):
        fail("tool_call", f"tool returned error: {result}")
    print(f"tool_call=PASS tool={call_name}")


if __name__ == "__main__":
    main()
