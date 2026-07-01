#!/usr/bin/env python3
import argparse
import base64
import hashlib
import hmac
import json
import time
import uuid


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def main() -> None:
    parser = argparse.ArgumentParser(description="Create a local cf-controlplane/cf-dataplane HS256 JWT.")
    parser.add_argument("--secret", default="my-test-key-but-now-longer-than-32-bytes")
    parser.add_argument("--subject", default="admin@example.com")
    parser.add_argument("--issuer", default="mcpgateway")
    parser.add_argument("--audience", default="mcpgateway-api")
    parser.add_argument("--ttl-seconds", type=int, default=86400)
    parser.add_argument("--server-id", default=None)
    args = parser.parse_args()

    now = int(time.time())
    header = {"alg": "HS256", "typ": "JWT"}
    payload = {
        "username": args.subject,
        "sub": args.subject,
        "jti": str(uuid.uuid4()),
        "token_use": "api",
        "iss": args.issuer,
        "aud": args.audience,
        "iat": now,
        "nbf": now,
        "exp": now + args.ttl_seconds,
        "teams": None,
        "user": {
            "email": args.subject,
            "full_name": "CLI User",
            "is_admin": True,
            "auth_provider": "cli",
        },
        "scopes": {
            "server_id": args.server_id,
            "permissions": ["servers.use", "tools.read", "tools.call"],
            "ip_restrictions": [],
            "time_restrictions": None,
        },
    }

    signing_input = ".".join(
        [
            b64url(json.dumps(header, separators=(",", ":")).encode("utf-8")),
            b64url(json.dumps(payload, separators=(",", ":")).encode("utf-8")),
        ]
    )
    signature = hmac.new(args.secret.encode("utf-8"), signing_input.encode("ascii"), hashlib.sha256).digest()
    print(f"{signing_input}.{b64url(signature)}")


if __name__ == "__main__":
    main()
