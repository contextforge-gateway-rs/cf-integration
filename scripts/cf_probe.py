#!/usr/bin/env python3
"""Probe one public MCP route using only the Python standard library."""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

from cf_jwt import DEFAULT_SECRET, DEFAULT_SUBJECT, make_token

ACCEPT = "application/json, text/event-stream"
MAX_BODY = 4 * 1024 * 1024
SAFE_TOOL_ARGUMENTS = {
    "echo": {"message": "cf-integration"},
    "fast_time_echo": {"message": "cf-integration"},
    "fast-time-echo": {"message": "cf-integration"},
    "get_system_time": {"timezone": "UTC"},
    "get-system-time": {"timezone": "UTC"},
    "fast-time-get_system_time": {"timezone": "UTC"},
    "fast_time_get_system_time": {"timezone": "UTC"},
    "fast-time-get-system-time": {"timezone": "UTC"},
}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, file_pointer, code, message, headers, new_url):
        return None


OPENER = urllib.request.build_opener(
    urllib.request.ProxyHandler({}),
    NoRedirect(),
)


def jsonrpc(method: str, params: dict | None = None, *, notification: bool = False) -> dict:
    payload = {"jsonrpc": "2.0", "method": method}
    if not notification:
        payload["id"] = str(uuid.uuid4())
    if params is not None:
        payload["params"] = params
    return payload


def parse_mcp_body(text: str, content_type: str):
    """Parse one JSON or SSE MCP response."""
    if "text/event-stream" not in content_type:
        return json.loads(text) if text else None
    message = None
    data: list[str] = []
    for line in [*text.splitlines(), ""]:
        if not line:
            if data:
                try:
                    message = json.loads("\n".join(data))
                except ValueError:
                    pass
                data.clear()
            continue
        if line.startswith("data:"):
            data.append(line.removeprefix("data:").lstrip())
    return message


def endpoint(base_url: str, topology: str, server_id: str) -> str:
    base = base_url.rstrip("/")
    if topology == "controlplane":
        return f"{base}/mcp"
    return f"{base}/servers/{urllib.parse.quote(server_id, safe='')}/mcp"


def _read_body(response) -> str:
    body = response.read(MAX_BODY + 1)
    if len(body) > MAX_BODY:
        raise RuntimeError("MCP response body exceeds safety limit")
    return body.decode("utf-8")


def _require_dataplane(headers) -> None:
    values = headers.get_all("X-CF-Integration-Backend", [])
    if values != ["dataplane"]:
        raise RuntimeError("dataplane backend identity check failed")


def post(
    url: str,
    payload: dict,
    token: str | None,
    session_id: str | None = None,
    protocol_version: str | None = None,
    *,
    require_dataplane: bool = False,
):
    request = urllib.request.Request(
        url, method="POST", data=json.dumps(payload).encode("utf-8")
    )
    request.add_header("Content-Type", "application/json")
    request.add_header("Accept", ACCEPT)
    if protocol_version:
        request.add_header("Mcp-Protocol-Version", protocol_version)
    if token:
        request.add_header("Authorization", f"Bearer {token}")
    if session_id:
        request.add_header("Mcp-Session-Id", session_id)
    try:
        timeout = int(os.environ.get("CF_PROBE_REQUEST_TIMEOUT", "30"))
        with OPENER.open(request, timeout=timeout) as response:
            if require_dataplane:
                _require_dataplane(response.headers)
            body = _read_body(response)
            return (
                response.status,
                response.headers,
                parse_mcp_body(body, response.headers.get("Content-Type", "")),
        )
    except urllib.error.HTTPError as error:
        if require_dataplane:
            _require_dataplane(error.headers)
        body = _read_body(error)
        message = None
        try:
            message = parse_mcp_body(body, error.headers.get("Content-Type", ""))
        except ValueError:
            pass
        return error.code, error.headers, message


def require_result(step: str, status: int, message, expected_id: str) -> dict:
    if (
        status != 200
        or not isinstance(message, dict)
        or message.get("jsonrpc") != "2.0"
        or message.get("id") != expected_id
        or "error" in message
    ):
        raise RuntimeError(f"{step}: status={status}, response={message!r}")
    result = message.get("result")
    if not isinstance(result, dict):
        raise RuntimeError(f"{step}: missing result object")
    return result


def run(topology: str, base_url: str, server_id: str) -> None:
    url = endpoint(base_url, topology, server_id)
    require_dataplane = topology == "dataplane"
    version = os.environ.get("MCP_SPEC_VERSION", "2025-11-25")
    initialize = jsonrpc(
        "initialize",
        {
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": "cf-integration", "version": "1.0"},
        },
    )
    initialize_id = initialize["id"]
    status, _, _ = post(
        url, initialize, None, require_dataplane=require_dataplane
    )
    if status != 401:
        raise RuntimeError(f"auth_negative: expected 401, got {status}")
    print(f"auth_negative=PASS status={status}")

    token = os.environ.get("MCPGATEWAY_BEARER_TOKEN") or make_token(
        os.environ.get("JWT_SECRET_KEY", DEFAULT_SECRET),
        os.environ.get("MCP_JWT_SUBJECT", DEFAULT_SUBJECT),
        admin=topology == "controlplane",
        server_id=server_id if topology == "dataplane" else None,
    )
    deadline = time.monotonic() + int(os.environ.get("CF_PROBE_CONFIG_TIMEOUT", "120"))
    while True:
        status, headers, message = post(
            url, initialize, token, require_dataplane=require_dataplane
        )
        if status == 200 or time.monotonic() >= deadline:
            break
        time.sleep(2)
    initialize_result = require_result("initialize", status, message, initialize_id)
    negotiated_version = initialize_result.get("protocolVersion")
    if not isinstance(negotiated_version, str) or not negotiated_version.strip():
        raise RuntimeError("initialize: missing negotiated protocolVersion")
    session_id = headers.get("Mcp-Session-Id")
    if not session_id or not session_id.strip():
        raise RuntimeError("initialize: missing Mcp-Session-Id")
    print(f"initialize=PASS status={status} session=present")

    status, _, message = post(
        url,
        jsonrpc("notifications/initialized", notification=True),
        token,
        session_id,
        negotiated_version,
        require_dataplane=require_dataplane,
    )
    if status != 202 or message is not None:
        raise RuntimeError(
            f"notifications/initialized: expected empty status 202, got {status}"
        )
    print("initialized=PASS status=202")

    tools_request = jsonrpc("tools/list", {})
    status, _, message = post(
        url,
        tools_request,
        token,
        session_id,
        negotiated_version,
        require_dataplane=require_dataplane,
    )
    tools = require_result("tools_list", status, message, tools_request["id"]).get(
        "tools"
    )
    if not isinstance(tools, list) or not tools:
        raise RuntimeError("tools_list: no tools returned")
    if any(
        not isinstance(tool, dict)
        or not isinstance(tool.get("name"), str)
        or not tool["name"].strip()
        for tool in tools
    ):
        raise RuntimeError("tools_list: every tool must have a nonempty name")
    print(f"tools_list=PASS count={len(tools)}")
    for tool in tools:
        print(f"tool={tool['name']}")

    for tool in tools:
        name = tool.get("name", "") if isinstance(tool, dict) else ""
        arguments = SAFE_TOOL_ARGUMENTS.get(name)
        if arguments is None:
            continue
        call_request = jsonrpc(
            "tools/call", {"name": name, "arguments": arguments}
        )
        status, _, message = post(
            url,
            call_request,
            token,
            session_id,
            negotiated_version,
            require_dataplane=require_dataplane,
        )
        result = require_result("tool_call", status, message, call_request["id"])
        if not isinstance(result.get("content"), list):
            raise RuntimeError(f"tool_call: {name} returned no content array")
        if "isError" in result and not isinstance(result["isError"], bool):
            raise RuntimeError(f"tool_call: {name} returned invalid isError")
        if result.get("isError", False):
            raise RuntimeError(f"tool_call: {name} returned isError=true")
        print(f"tool_call=PASS tool={name}")
        return
    print("tool_call=SKIP no allowlisted fixture tool available")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--topology", choices=("controlplane", "dataplane"), required=True)
    args = parser.parse_args()
    server_id = (
        os.environ.get("MCP_SERVER_ID")
        or os.environ.get("MCP_VIRTUAL_SERVER_ID")
        or os.environ.get("CF_FAST_TIME_SERVER_ID")
        or "9779b6698cbd4b4995ee04a4fab38737"
    )
    try:
        run(
            args.topology,
            os.environ.get("MCP_CLI_BASE_URL", "http://127.0.0.1:8080"),
            server_id,
        )
    except Exception as error:  # noqa: BLE001 - CLI boundary
        print(f"probe=FAIL {error}", file=sys.stderr)
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
