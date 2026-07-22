from __future__ import annotations

import base64
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.request
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

import auth_proxy
import cf_jwt
import cf_probe
import conformance
import validate_compose


class MakeCliTests(unittest.TestCase):
    def test_make_help_exposes_each_workflow(self):
        output = subprocess.check_output(
            ["make", "help"], cwd=ROOT, text=True
        )
        for target in (
            "checkout",
            "up",
            "down",
            "reset",
            "status",
            "logs",
            "config",
            "probe",
            "load",
            "smoke",
            "live",
            "conformance",
            "conformance-report",
            "inspect",
            "token",
        ):
            self.assertIn(target, output)

    def test_repository_has_no_rust_workspace_surface(self):
        self.assertFalse((ROOT / "Cargo.toml").exists())
        self.assertFalse((ROOT / "Cargo.lock").exists())
        tracked_rust = [
            name
            for name in subprocess.check_output(
                ["git", "ls-files", "*.rs"], cwd=ROOT, text=True
            ).splitlines()
            if (ROOT / name).is_file()
        ]
        self.assertEqual(tracked_rust, [])
        self.assertNotIn("cargo", (ROOT / "Makefile").read_text().lower())

    def test_scoped_token_contains_the_fixed_server_scope(self):
        token = cf_jwt.make_token("secret", "user@example.com", server_id="server-1")
        payload_segment = token.split(".")[1]
        payload_segment += "=" * (-len(payload_segment) % 4)
        payload = json.loads(base64.urlsafe_b64decode(payload_segment))

        self.assertEqual(payload["sub"], "user@example.com")
        self.assertEqual(payload["scopes"]["server_id"], "server-1")
        self.assertEqual(payload["token_use"], "api")
        self.assertEqual(payload["user"]["full_name"], "CLI User")

    def test_invalid_load_settings_fail_before_stack_orchestration(self):
        environment = {**os.environ, "USERS": "0", "TOPOLOGY": "dataplane"}
        completed = subprocess.run(
            [str(ROOT / "scripts" / "cf-integration.sh"), "load"],
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )

        self.assertEqual(completed.returncode, 1)
        self.assertIn("USERS must be an integer greater than zero", completed.stderr)
        self.assertNotIn("Cloning", completed.stdout)

    def test_probe_endpoints_preserve_topology_and_encode_server_ids(self):
        self.assertEqual(
            cf_probe.endpoint("http://127.0.0.1:8080", "controlplane", "ignored"),
            "http://127.0.0.1:8080/mcp",
        )
        self.assertEqual(
            cf_probe.endpoint("http://127.0.0.1:8080/", "dataplane", "a/b c"),
            "http://127.0.0.1:8080/servers/a%2Fb%20c/mcp",
        )

    def test_auth_proxy_injects_token_and_accepts_dataplane_identity(self):
        observed = {}

        class Upstream(http.server.BaseHTTPRequestHandler):
            def do_POST(self):  # noqa: N802
                length = int(self.headers.get("Content-Length", "0"))
                observed["authorization"] = self.headers.get("Authorization")
                observed["host"] = self.headers.get("Host")
                observed["body"] = self.rfile.read(length)
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("X-CF-Integration-Backend", "dataplane")
                self.end_headers()
                self.wfile.write(b'{"ok":true}')

            def log_message(self, _format, *_args):
                return

        upstream = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
        upstream_thread = threading.Thread(target=upstream.serve_forever, daemon=True)
        upstream_thread.start()
        host, port = upstream.server_address
        proxy = auth_proxy.ProxyServer(
            f"http://{host}:{port}/servers/server-1/mcp", "secret-token"
        )
        proxy_thread = threading.Thread(target=proxy.serve_forever, daemon=True)
        proxy_thread.start()
        try:
            request = urllib.request.Request(
                proxy.url, method="POST", data=b'{"jsonrpc":"2.0"}'
            )
            request.add_header("Host", "rebinding.invalid")
            with urllib.request.urlopen(request, timeout=5) as response:
                self.assertEqual(json.loads(response.read()), {"ok": True})
        finally:
            proxy.shutdown()
            proxy.server_close()
            proxy_thread.join(timeout=5)
            upstream.shutdown()
            upstream.server_close()
            upstream_thread.join(timeout=5)

        self.assertEqual(observed["authorization"], "Bearer secret-token")
        self.assertEqual(observed["host"], "rebinding.invalid")
        self.assertEqual(observed["body"], b'{"jsonrpc":"2.0"}')

    def test_auth_proxy_child_environment_does_not_expose_credentials(self):
        with mock.patch.dict(
            os.environ,
            {
                "PATH": "/usr/bin",
                "AUTH_PROXY_TOKEN": "proxy-secret",
                "MCPGATEWAY_BEARER_TOKEN": "gateway-secret",
                "JWT_SECRET_KEY": "jwt-secret",
            },
            clear=True,
        ):
            environment = auth_proxy._child_environment()

        self.assertEqual(environment, {"PATH": "/usr/bin"})

    def test_compose_contract_accepts_profile_gated_fast_test(self):
        config = {
            "services": {
                "fast_time_server": {"image": "fast-time:current"},
                "fast_test_server": {"profiles": ["testing"], "image": "fast-time:current"},
                "register_fast_test": {"profiles": ["testing"]},
                "register_fast_time": {
                    "command": "wait http://fast_time_server:9080/health register http://fast_time_server:9080/mcp"
                },
            }
        }
        self.assertEqual(validate_compose.violations(config, "fast-time:current"), [])

    def test_conformance_report_compares_script_generated_lanes(self):
        metadata = {
            "oracle": conformance.PACKAGE,
            "spec_version": "2025-11-25",
            "server_era": "dual",
            "suite": "all",
            "fixture": {
                "repository": conformance.REPOSITORY,
                "revision": conformance.REVISION,
                "server_id": conformance.SERVER_ID,
            },
        }
        with tempfile.TemporaryDirectory() as directory:
            artifact_root = Path(directory) / "artifacts"
            for lane, status in (
                ("fixture-direct", "SUCCESS"),
                ("controlplane", "FAILURE"),
                ("dataplane", "SUCCESS"),
            ):
                root = conformance.lane_root(artifact_root, lane)
                checks = root / "official" / "server-ping-2026-07-22T00-00-00-000Z" / "checks.json"
                checks.parent.mkdir(parents=True)
                checks.write_text(json.dumps([{"id": "ping", "status": status}]))
                (root / "metadata.json").write_text(
                    json.dumps({**metadata, "target": lane})
                )
                (root / "complete").write_text("complete\n")

            report = conformance.report(artifact_root, Path(directory) / "reports")
            rendered = report.read_text()

        self.assertIn("| ping | compliant | failure | compliant | control-plane only failure |", rendered)


if __name__ == "__main__":
    unittest.main()
