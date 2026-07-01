#!/usr/bin/env python3
"""Register the integration Fast Time MCP server with cf-controlplane."""

from __future__ import annotations

import base64
import hashlib
import hmac
import json
import os
import sys
import time
import urllib.error
import urllib.request
import uuid

import msgpack
import redis


GATEWAY_BASE_URL = os.environ.get("CF_CONTROLPLANE_INTERNAL_URL", "http://gateway:4444")
FAST_TIME_BASE_URL = os.environ.get("CF_FAST_TIME_INTERNAL_URL", "http://fast_time_server:8080")
FAST_TIME_MCP_URL = os.environ.get("CF_FAST_TIME_MCP_URL", f"{FAST_TIME_BASE_URL}/mcp")
JWT_SECRET_KEY = os.environ.get("JWT_SECRET_KEY", "my-test-key-but-now-longer-than-32-bytes")
ADMIN_EMAIL = os.environ.get("PLATFORM_ADMIN_EMAIL", "admin@example.com")
VIRTUAL_SERVER_ID = os.environ.get("CF_FAST_TIME_SERVER_ID", "9779b6698cbd4b4995ee04a4fab38737")
REDIS_URL = os.environ.get("REDIS_URL", "redis://redis:6379/0")
DATAPLANE_CONFIG_TTL = int(os.environ.get("CF_DATAPLANE_CONFIG_TTL", "3600"))


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def make_token() -> str:
    now = int(time.time())
    header = {"alg": "HS256", "typ": "JWT"}
    payload = {
        "username": ADMIN_EMAIL,
        "sub": ADMIN_EMAIL,
        "jti": str(uuid.uuid4()),
        "token_use": "api",
        "iss": "mcpgateway",
        "aud": "mcpgateway-api",
        "iat": now,
        "nbf": now,
        "exp": now + 7 * 24 * 60 * 60,
        "teams": None,
        "user": {
            "email": ADMIN_EMAIL,
            "full_name": "CLI User",
            "is_admin": True,
            "auth_provider": "cli",
        },
    }
    signing_input = ".".join(
        [
            b64url(json.dumps(header, separators=(",", ":")).encode("utf-8")),
            b64url(json.dumps(payload, separators=(",", ":")).encode("utf-8")),
        ]
    )
    sig = hmac.new(JWT_SECRET_KEY.encode("utf-8"), signing_input.encode("ascii"), hashlib.sha256).digest()
    return f"{signing_input}.{b64url(sig)}"


TOKEN = make_token()


def request_json(method: str, path: str, data: dict | None = None, timeout: int = 60):
    req = urllib.request.Request(f"{GATEWAY_BASE_URL}{path}", method=method)
    req.add_header("Authorization", f"Bearer {TOKEN}")
    req.add_header("Content-Type", "application/json")
    if data is not None:
        req.data = json.dumps(data).encode("utf-8")
    with urllib.request.urlopen(req, timeout=timeout) as response:
        body = response.read().decode("utf-8")
        return json.loads(body) if body else None


def request_json_with_retry(
    method: str,
    path: str,
    data: dict | None = None,
    retries: int = 30,
    delay: int = 2,
    retry_statuses: tuple[int, ...] = (401, 409, 502, 503),
):
    for attempt in range(1, retries + 1):
        try:
            return request_json(method, path, data)
        except urllib.error.HTTPError as exc:
            if exc.code in retry_statuses and attempt < retries:
                print(f"Retrying {method} {path} after HTTP {exc.code} ({attempt}/{retries})")
                time.sleep(delay)
                continue
            raise
        except Exception as exc:
            if attempt < retries:
                print(f"Retrying {method} {path} after {exc} ({attempt}/{retries})")
                time.sleep(delay)
                continue
            raise


def wait_url(name: str, url: str, retries: int = 60) -> None:
    for attempt in range(1, retries + 1):
        try:
            with urllib.request.urlopen(url, timeout=2) as response:
                if response.status == 200:
                    print(f"{name} is healthy")
                    return
        except Exception:
            pass
        print(f"Waiting for {name}... ({attempt}/{retries})")
        time.sleep(2)
    raise SystemExit(f"{name} failed to become healthy")


def delete_if_exists(kind: str, path: str) -> None:
    try:
        request_json_with_retry("DELETE", path, retries=10)
        print(f"Deleted existing {kind}")
    except urllib.error.HTTPError as exc:
        if exc.code != 404:
            print(f"Note: could not delete existing {kind}: HTTP {exc.code}")
    except Exception as exc:
        print(f"Note: could not delete existing {kind}: {exc}")


def items_for_gateway(path: str, gateway_id: str) -> list[dict]:
    rows = request_json_with_retry("GET", path) or []
    return [
        row
        for row in rows
        if row.get("gatewayId") == gateway_id or row.get("gateway_id") == gateway_id
    ]


def publish_dataplane_config(
    gateway_id: str,
    tool_names: list[str],
    resource_names: list[str],
    prompt_names: list[str],
) -> None:
    config = {
        "virtual_hosts": {
            VIRTUAL_SERVER_ID: {
                "backends": {
                    gateway_id: {
                        "name": "fast_time",
                        "url": FAST_TIME_MCP_URL,
                        "transport": "STREAMABLEHTTP",
                        "passthrough_headers": [],
                        "allowed_tool_names": tool_names,
                        "allowed_resource_names": resource_names,
                        "allowed_prompt_names": prompt_names,
                    }
                }
            }
        }
    }
    client = redis.Redis.from_url(REDIS_URL)
    key = msgpack.dumps(("UserConfig", ADMIN_EMAIL), use_bin_type=True)
    value = msgpack.dumps(config, use_bin_type=True)
    client.set(key, value, ex=DATAPLANE_CONFIG_TTL)
    client.close()
    print(f"Published dataplane config for {ADMIN_EMAIL} with ttl={DATAPLANE_CONFIG_TTL}s")


def main() -> int:
    print("Registering Fast Time MCP server")
    print(f"Fast Time URL: {FAST_TIME_MCP_URL}")

    wait_url("gateway", f"{GATEWAY_BASE_URL}/health")
    wait_url("fast_time_server", f"{FAST_TIME_BASE_URL}/health", retries=30)

    for attempt in range(1, 61):
        try:
            gateways = request_json("GET", "/gateways")
            print(f"Authenticated gateway readiness confirmed ({len(gateways)} gateways visible)")
            break
        except Exception as exc:
            print(f"Authenticated gateway not ready ({attempt}/60): {exc}")
            time.sleep(2)
    else:
        raise SystemExit("Gateway authenticated readiness check failed")

    delete_if_exists("Fast Time virtual server", f"/servers/{VIRTUAL_SERVER_ID}")

    gateways = request_json_with_retry("GET", "/gateways") or []
    for gateway in gateways:
        if gateway.get("name") == "fast_time":
            delete_if_exists("Fast Time gateway", f"/gateways/{gateway['id']}")

    result = request_json_with_retry(
        "POST",
        "/gateways",
        {
            "name": "fast_time",
            "url": FAST_TIME_MCP_URL,
            "transport": "STREAMABLEHTTP",
        },
        retries=20,
    )
    gateway_id = result.get("id") if isinstance(result, dict) else None
    if not gateway_id:
        raise SystemExit(f"Gateway registration did not return an id: {result}")
    print(f"Registered Fast Time gateway {gateway_id}")

    try:
        refresh = request_json_with_retry(
            "POST",
            f"/gateways/{gateway_id}/tools/refresh?include_resources=true&include_prompts=true",
            retries=20,
        )
        print(f"Refresh response: {refresh}")
    except Exception as exc:
        print(f"Note: manual refresh did not complete immediately: {exc}")

    for attempt in range(1, 61):
        tool_items = items_for_gateway("/tools", gateway_id)
        if tool_items:
            print(f"Found {len(tool_items)} tools from Fast Time gateway")
            break
        print(f"Waiting for tool sync... ({attempt}/60)")
        time.sleep(1)
    else:
        raise SystemExit("Fast Time gateway did not sync tools")

    resource_items = items_for_gateway("/resources", gateway_id)
    prompt_items = items_for_gateway("/prompts", gateway_id)
    tool_ids = [item["id"] for item in tool_items]
    resource_ids = [item["id"] for item in resource_items]
    prompt_ids = [item["id"] for item in prompt_items]
    tool_names = [item["name"] for item in tool_items]
    resource_names = [item["name"] for item in resource_items]
    prompt_names = [item["name"] for item in prompt_items]
    print(f"Found {len(resource_ids)} resources and {len(prompt_ids)} prompts from Fast Time gateway")

    payload = {
        "server": {
            "id": VIRTUAL_SERVER_ID,
            "name": "Fast Time Server",
            "description": "Virtual server exposing Fast Time MCP tools",
            "associated_tools": tool_ids,
            "associated_resources": resource_ids,
            "associated_prompts": prompt_ids,
        }
    }
    server = request_json_with_retry("POST", "/servers", payload)
    print(f"Created Fast Time virtual server: {server.get('id') if isinstance(server, dict) else server}")
    publish_dataplane_config(gateway_id, tool_names, resource_names, prompt_names)
    print("Fast Time registration complete")
    return 0


if __name__ == "__main__":
    sys.exit(main())
