#!/usr/bin/env python3
"""Run a command behind a random-path loopback bearer-injection proxy."""

from __future__ import annotations

import argparse
import http.client
import http.server
import os
import secrets
import subprocess
import threading
import urllib.parse

HOP_BY_HOP = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
}
MAX_BODY = 4 * 1024 * 1024
CHILD_ENV_ALLOWLIST = (
    "PATH",
    "HOME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "XDG_CACHE_HOME",
    "NPM_CONFIG_CACHE",
    "npm_config_cache",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
)


def _connection_headers(values: list[str]) -> set[str]:
    return {
        name.strip().lower()
        for value in values
        for name in value.split(",")
        if name.strip()
    }


def _child_environment() -> dict[str, str]:
    return {key: os.environ[key] for key in CHILD_ENV_ALLOWLIST if key in os.environ}


class ProxyServer(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, upstream: str, token: str):
        parsed = urllib.parse.urlsplit(upstream)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("AUTH_PROXY_UPSTREAM must be an absolute HTTP(S) URL")
        if parsed.username or parsed.password or parsed.fragment:
            raise ValueError("AUTH_PROXY_UPSTREAM cannot contain credentials or a fragment")
        if not token or "\r" in token or "\n" in token:
            raise ValueError("AUTH_PROXY_TOKEN is not a valid bearer credential")
        self.upstream = parsed
        self.token = token
        self.proxy_path = f"/mcp-auth/{secrets.token_hex(16)}"
        segments = parsed.path.split("/")
        self.require_dataplane = (
            len(segments) == 4
            and segments[0] == ""
            and segments[1] == "servers"
            and bool(segments[2])
            and segments[3] == "mcp"
        )
        super().__init__(("127.0.0.1", 0), ProxyHandler)

    @property
    def url(self) -> str:
        host, port = self.server_address
        return f"http://{host}:{port}{self.proxy_path}"


class ProxyHandler(http.server.BaseHTTPRequestHandler):
    server: ProxyServer
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:  # noqa: N802 - stdlib handler API
        self._forward()

    def do_POST(self) -> None:  # noqa: N802 - stdlib handler API
        self._forward()

    def do_DELETE(self) -> None:  # noqa: N802 - stdlib handler API
        self._forward()

    def _forward(self) -> None:
        if self.path != self.server.proxy_path:
            self.send_error(404)
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            self.send_error(400)
            return
        if length > MAX_BODY:
            self.send_error(413)
            return
        body = self.rfile.read(length) if length else None
        upstream = self.server.upstream
        connection_class = (
            http.client.HTTPSConnection
            if upstream.scheme == "https"
            else http.client.HTTPConnection
        )
        port = upstream.port or (443 if upstream.scheme == "https" else 80)
        connection = connection_class(upstream.hostname, port, timeout=3600)
        connection_headers = _connection_headers(
            self.headers.get_all("Connection", [])
        )
        headers = {
            key: value
            for key, value in self.headers.items()
            if key.lower()
            not in HOP_BY_HOP
            | connection_headers
            | {"authorization", "content-length"}
        }
        host, proxy_port = self.server.server_address
        if headers.get("Host") == f"{host}:{proxy_port}":
            headers.pop("Host")
        headers["Authorization"] = f"Bearer {self.server.token}"
        if body is not None:
            headers["Content-Length"] = str(len(body))
        target = urllib.parse.urlunsplit(("", "", upstream.path or "/", upstream.query, ""))
        try:
            connection.request(self.command, target, body=body, headers=headers)
            response = connection.getresponse()
            backend_values = [
                value
                for key, value in response.getheaders()
                if key.lower() == "x-cf-integration-backend"
            ]
            if self.server.require_dataplane and backend_values != ["dataplane"]:
                response.read()
                self.send_error(502, "dataplane backend identity check failed")
                return

            self.send_response(response.status)
            response_headers = response.getheaders()
            connection_headers = _connection_headers(
                [value for key, value in response_headers if key.lower() == "connection"]
            )
            for key, value in response_headers:
                if key.lower() not in HOP_BY_HOP | connection_headers | {"content-length"}:
                    self.send_header(key, value)
            self.send_header("Connection", "close")
            self.end_headers()
            while chunk := response.read(64 * 1024):
                self.wfile.write(chunk)
                self.wfile.flush()
            self.close_connection = True
        except (OSError, http.client.HTTPException):
            if not self.wfile.closed:
                self.send_error(502)
        finally:
            connection.close()

    def log_message(self, _format: str, *_args) -> None:
        return


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.command[:1] == ["--"]:
        args.command = args.command[1:]
    if not args.command:
        parser.error("a command after -- is required")

    upstream = os.environ.get("AUTH_PROXY_UPSTREAM", "")
    token = os.environ.get("AUTH_PROXY_TOKEN", "")
    if not upstream or not token:
        parser.error("AUTH_PROXY_UPSTREAM and AUTH_PROXY_TOKEN are required")
    server = ProxyServer(upstream, token)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    command = [argument.replace("{proxy_url}", server.url) for argument in args.command]
    try:
        completed = subprocess.run(
            command,
            check=False,
            env=_child_environment(),
        )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    raise SystemExit(completed.returncode)


if __name__ == "__main__":
    main()
