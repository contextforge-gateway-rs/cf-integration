#!/usr/bin/env python3
"""Local cf-controlplane/cf-dataplane HS256 JWT helper.

Usable as a CLI or imported for its make_token() function (the compose
overlay mounts this file as cf_jwt.py next to consumers).
"""
import argparse
import base64
import hashlib
import hmac
import json
import time
import uuid


DEFAULT_SCOPES = {
    "server_id": None,
    "permissions": ["servers.use", "tools.read", "tools.call"],
    "ip_restrictions": [],
    "time_restrictions": None,
}


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def make_token(
    secret: str,
    subject: str,
    *,
    issuer: str = "mcpgateway",
    audience: str = "mcpgateway-api",
    ttl_seconds: int = 86400,
    scopes: dict | None = None,
) -> str:
    now = int(time.time())
    header = {"alg": "HS256", "typ": "JWT"}
    payload = {
        "username": subject,
        "sub": subject,
        "jti": str(uuid.uuid4()),
        "token_use": "api",
        "iss": issuer,
        "aud": audience,
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
    if scopes is not None:
        payload["scopes"] = scopes

    signing_input = ".".join(
        [
            b64url(json.dumps(header, separators=(",", ":")).encode("utf-8")),
            b64url(json.dumps(payload, separators=(",", ":")).encode("utf-8")),
        ]
    )
    signature = hmac.new(secret.encode("utf-8"), signing_input.encode("ascii"), hashlib.sha256).digest()
    return f"{signing_input}.{b64url(signature)}"


def main() -> None:
    parser = argparse.ArgumentParser(description="Create a local cf-controlplane/cf-dataplane HS256 JWT.")
    parser.add_argument("--secret", default="my-test-key-but-now-longer-than-32-bytes")
    parser.add_argument("--subject", default="admin@example.com")
    parser.add_argument("--issuer", default="mcpgateway")
    parser.add_argument("--audience", default="mcpgateway-api")
    parser.add_argument("--ttl-seconds", type=int, default=86400)
    parser.add_argument("--server-id", default=None)
    args = parser.parse_args()

    scopes = {**DEFAULT_SCOPES, "server_id": args.server_id}
    print(
        make_token(
            args.secret,
            args.subject,
            issuer=args.issuer,
            audience=args.audience,
            ttl_seconds=args.ttl_seconds,
            scopes=scopes,
        )
    )


if __name__ == "__main__":
    main()
