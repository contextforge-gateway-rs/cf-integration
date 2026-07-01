"""Locust load test for the public nginx -> cf-dataplane MCP route.

Harness-owned replacement for the upstream locustfile_mcp_protocol.py, which
sends ``Accept: application/json`` and gets HTTP 406 from the streamable HTTP
endpoint. This file negotiates ``application/json, text/event-stream`` and
parses either response form.

Env:
  MCP_SERVER_ID / MCP_VIRTUAL_SERVER_ID  virtual server id (required)
  MCPGATEWAY_BEARER_TOKEN                bearer token (required)
  MCP_TOOL_NAMES                         optional comma-separated tools to call
"""
from __future__ import annotations

import json
import os
import random
import uuid

from locust import HttpUser, between, task

MCP_SERVER_ID = os.environ.get("MCP_SERVER_ID") or os.environ.get("MCP_VIRTUAL_SERVER_ID", "")
BEARER_TOKEN = os.environ.get("MCPGATEWAY_BEARER_TOKEN", "")
TOOL_NAMES = [name.strip() for name in os.environ.get("MCP_TOOL_NAMES", "").split(",") if name.strip()]
PROTOCOL_VERSION = "2025-06-18"


def _jsonrpc(method: str, params: dict | None = None) -> dict:
    payload = {"jsonrpc": "2.0", "id": str(uuid.uuid4()), "method": method}
    if params is not None:
        payload["params"] = params
    return payload


def _parse_mcp_body(text: str, content_type: str):
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


class MCPDataplaneUser(HttpUser):
    """Drives initialize -> tools/list -> tools/call against /servers/{id}/mcp."""

    wait_time = between(0.05, 0.2)

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self._session_id: str | None = None
        self._tool_names: list[str] = list(TOOL_NAMES)

    def on_start(self):
        if not MCP_SERVER_ID:
            raise RuntimeError("MCP_SERVER_ID or MCP_VIRTUAL_SERVER_ID is required")
        if not BEARER_TOKEN:
            raise RuntimeError("MCPGATEWAY_BEARER_TOKEN is required")
        result = self._mcp_request(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "cf-integration-locust", "version": "1.0"},
            },
            name="MCP initialize",
        )
        if result is None:
            return
        if not self._tool_names:
            listed = self._mcp_request("tools/list", {}, name="MCP tools/list")
            if listed:
                self._tool_names = [tool["name"] for tool in listed.get("tools", [])]

    def _headers(self) -> dict[str, str]:
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
            "Authorization": f"Bearer {BEARER_TOKEN}",
            "Mcp-Protocol-Version": PROTOCOL_VERSION,
        }
        if self._session_id:
            headers["Mcp-Session-Id"] = self._session_id
        return headers

    def _mcp_request(self, method: str, params: dict | None, name: str) -> dict | None:
        """Send an MCP JSON-RPC request; return the result field or None."""
        with self.client.post(
            f"/servers/{MCP_SERVER_ID}/mcp",
            data=json.dumps(_jsonrpc(method, params)),
            headers=self._headers(),
            name=name,
            catch_response=True,
        ) as response:
            session_id = response.headers.get("Mcp-Session-Id") if response.headers else None
            if session_id:
                self._session_id = session_id

            if response.status_code != 200:
                response.failure(f"HTTP {response.status_code}")
                return None
            try:
                message = _parse_mcp_body(response.text, response.headers.get("Content-Type", ""))
            except ValueError as exc:
                response.failure(f"Invalid body: {exc}")
                return None
            if not isinstance(message, dict):
                response.failure("No JSON-RPC message in response")
                return None
            if "error" in message:
                error = message["error"]
                response.failure(f"JSON-RPC error {error.get('code', '?')}: {error.get('message', '?')}")
                return None
            response.success()
            return message.get("result") or {}

    @task(5)
    def tools_list(self):
        self._mcp_request("tools/list", {}, name="MCP tools/list")

    @task(10)
    def tools_call(self):
        candidates = [name for name in self._tool_names if name.endswith(("echo", "get_system_time"))]
        if not candidates:
            return
        tool = random.choice(candidates)
        args = {"message": "cf-integration-locust"} if tool.endswith("echo") else {"timezone": "UTC"}
        self._mcp_request("tools/call", {"name": tool, "arguments": args}, name=f"MCP tools/call {tool.rsplit('-', 1)[-1]}")

    @task(2)
    def ping(self):
        self._mcp_request("ping", None, name="MCP ping")
