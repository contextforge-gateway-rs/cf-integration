#!/usr/bin/env python3
"""Create the local HS256 tokens used by the integration harness."""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
import time
import uuid

DEFAULT_SECRET = "my-test-key-but-now-longer-than-32-bytes"
DEFAULT_SUBJECT = "admin@example.com"
DEFAULT_SCOPES = {
    "server_id": None,
    "permissions": ["servers.read", "servers.use", "tools.read", "tools.call"],
    "ip_restrictions": [],
    "time_restrictions": None,
}


def _b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def make_token(
    secret: str,
    subject: str,
    *,
    admin: bool = False,
    server_id: str | None = None,
    ttl_seconds: int = 86_400,
) -> str:
    """Return a control-plane admin token or dataplane-scoped API token."""
    now = int(time.time())
    header = {"alg": "HS256", "typ": "JWT"}
    payload = {
        "username": subject,
        "sub": subject,
        "jti": str(uuid.uuid4()),
        "token_use": "session" if admin else "api",
        "iss": "mcpgateway",
        "aud": "mcpgateway-api",
        "iat": now,
        "nbf": now,
        "exp": now + ttl_seconds,
        "teams": None,
        "user": {
            "email": subject,
            "full_name": "CLI User",
            "is_admin": True,
            "auth_provider": "cli",
        },
    }
    if not admin:
        payload["scopes"] = {**DEFAULT_SCOPES, "server_id": server_id}

    signing_input = ".".join(
        _b64url(json.dumps(part, separators=(",", ":")).encode())
        for part in (header, payload)
    )
    signature = hmac.new(
        secret.encode(), signing_input.encode("ascii"), hashlib.sha256
    ).digest()
    return f"{signing_input}.{_b64url(signature)}"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", choices=("scoped", "admin"), default="scoped")
    parser.add_argument("--server-id")
    parser.add_argument("--ttl-seconds", type=int, default=86_400)
    args = parser.parse_args()
    if args.ttl_seconds <= 0:
        parser.error("--ttl-seconds must be greater than zero")
    if args.kind == "admin" and args.server_id:
        parser.error("--server-id is only valid with --kind scoped")

    secret = os.environ.get("JWT_SECRET_KEY", DEFAULT_SECRET)
    subject = os.environ.get("MCP_JWT_SUBJECT", DEFAULT_SUBJECT)
    print(
        make_token(
            secret,
            subject,
            admin=args.kind == "admin",
            server_id=args.server_id,
            ttl_seconds=args.ttl_seconds,
        )
    )


if __name__ == "__main__":
    main()
