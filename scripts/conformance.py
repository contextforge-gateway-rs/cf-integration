#!/usr/bin/env python3
"""Provision, run, and report the pinned official MCP conformance workflow."""

from __future__ import annotations

import argparse
import ipaddress
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

PACKAGE = "@modelcontextprotocol/conformance@0.2.0-alpha.9"
REPOSITORY = "https://github.com/modelcontextprotocol/conformance"
REVISION = "794dcab99ed1ef2b89607be9999574140ea5c96e"
BACKEND_URL = "http://mcp_conformance_server:3000/mcp"
GATEWAY_NAME = "_"
GATEWAY_DESCRIPTION = "Official MCP conformance fixture"
SERVER_ID = "3f33286667d34b65a31c3bafd30e4c21"
SERVER_NAME = "Official MCP Conformance Server"
SERVER_DESCRIPTION = "Virtual server for the pinned official MCP conformance fixture."
REQUIRED = {
    "tools": ("name", "test_simple_text"),
    "resources": ("uri", "test://static-text"),
    "prompts": ("name", "test_simple_prompt"),
}
LANES = ("fixture-direct", "controlplane", "dataplane")
MAX_API_BODY = 8 * 1024 * 1024
NPM_ENV_ALLOWLIST = (
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


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, file_pointer, code, message, headers, new_url):
        return None


OPENER = urllib.request.build_opener(
    urllib.request.ProxyHandler({}),
    NoRedirect(),
)


def npm_environment() -> dict[str, str]:
    return {key: os.environ[key] for key in NPM_ENV_ALLOWLIST if key in os.environ}


def read_bounded(response) -> bytes:
    body = response.read(MAX_API_BODY + 1)
    if len(body) > MAX_API_BODY:
        raise RuntimeError("control-plane API response exceeds safety limit")
    return body


class ApiClient:
    def __init__(self, base_url: str, token: str):
        parsed = urllib.parse.urlsplit(base_url)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("base URL must be an absolute HTTP(S) origin")
        if (
            parsed.username
            or parsed.password
            or parsed.path not in {"", "/"}
            or parsed.query
            or parsed.fragment
        ):
            raise ValueError("base URL must contain only an HTTP origin")
        try:
            loopback = ipaddress.ip_address(parsed.hostname).is_loopback
        except ValueError:
            loopback = False
        if not loopback:
            raise ValueError("official conformance requires a loopback base URL")
        self.base_url = base_url.rstrip("/")
        self.token = token

    def request(self, method: str, path: str, payload=None, *, missing_ok=False):
        body = None if payload is None else json.dumps(payload).encode("utf-8")
        request = urllib.request.Request(
            f"{self.base_url}{path}", method=method, data=body
        )
        request.add_header("Authorization", f"Bearer {self.token}")
        request.add_header("Content-Type", "application/json")
        try:
            with OPENER.open(request, timeout=30) as response:
                data = read_bounded(response)
                return json.loads(data) if data else None
        except urllib.error.HTTPError as error:
            if missing_ok and error.code == 404:
                return None
            detail = read_bounded(error).decode("utf-8", errors="replace")
            detail = detail.replace(self.token, "<redacted>")
            raise RuntimeError(f"{method} {path} failed with HTTP {error.code}: {detail}") from None

    def collection(self, path: str) -> list[dict]:
        value = self.request("GET", path)
        if isinstance(value, list):
            return value
        if isinstance(value, dict):
            for key in ("items", "data", path.strip("/")):
                if isinstance(value.get(key), list):
                    return value[key]
        raise RuntimeError(f"GET {path} did not return a collection")


def _gateway_owned(record: dict) -> bool:
    return (
        record.get("name") == GATEWAY_NAME
        and record.get("url") == BACKEND_URL
        and record.get("transport") == "STREAMABLEHTTP"
        and record.get("description") == GATEWAY_DESCRIPTION
    )


def _server_owned(record: dict) -> bool:
    return (
        record.get("id") == SERVER_ID
        and record.get("name") == SERVER_NAME
        and record.get("description") == SERVER_DESCRIPTION
    )


def cleanup(client: ApiClient, gateway_id: str | None = None) -> None:
    matching_servers = [s for s in client.collection("/servers") if s.get("id") == SERVER_ID]
    if any(not _server_owned(server) for server in matching_servers):
        raise RuntimeError("reserved conformance server ID is owned by another resource")
    if matching_servers:
        client.request("DELETE", f"/servers/{SERVER_ID}", missing_ok=True)

    gateways = client.collection("/gateways")
    if gateway_id:
        selected = [gateway for gateway in gateways if gateway.get("id") == gateway_id]
    else:
        selected = [gateway for gateway in gateways if gateway.get("name") == GATEWAY_NAME]
    if any(not _gateway_owned(gateway) for gateway in selected):
        raise RuntimeError("reserved conformance gateway is owned by another resource")
    for gateway in selected:
        client.request("DELETE", f"/gateways/{gateway['id']}", missing_ok=True)


def provision(client: ApiClient) -> str:
    cleanup(client)
    gateway = client.request(
        "POST",
        "/gateways",
        {
            "name": GATEWAY_NAME,
            "url": BACKEND_URL,
            "transport": "STREAMABLEHTTP",
            "description": GATEWAY_DESCRIPTION,
        },
    )
    if not isinstance(gateway, dict) or not gateway.get("id"):
        raise RuntimeError("gateway creation did not return an ID")
    gateway_id = str(gateway["id"])
    client.request(
        "POST",
        f"/gateways/{gateway_id}/tools/refresh?include_resources=true&include_prompts=true",
    )

    catalogs: dict[str, list[dict]] = {}
    missing = list(REQUIRED.values())
    for _ in range(40):
        catalogs = {
            name: [
                record
                for record in client.collection(f"/{name}")
                if record.get("gateway_id", record.get("gatewayId")) == gateway_id
            ]
            for name in REQUIRED
        }
        missing = [
            (field, expected)
            for name, (field, expected) in REQUIRED.items()
            if not any(record.get(field) == expected for record in catalogs[name])
        ]
        if not missing:
            break
        time.sleep(0.25)
    if missing:
        cleanup(client, gateway_id)
        raise RuntimeError(f"fixture catalogs are missing required identities: {missing}")

    client.request(
        "POST",
        "/servers",
        {
            "server": {
                "id": SERVER_ID,
                "name": SERVER_NAME,
                "description": SERVER_DESCRIPTION,
                "associated_tools": [record["id"] for record in catalogs["tools"]],
                "associated_resources": [record["id"] for record in catalogs["resources"]],
                "associated_prompts": [record["id"] for record in catalogs["prompts"]],
            }
        },
    )
    return gateway_id


def lane_root(artifact_root: Path, lane: str) -> Path:
    if lane not in LANES:
        raise ValueError(f"unknown conformance lane: {lane}")
    return artifact_root / "conformance" / lane


def run_lane(args) -> int:
    root = lane_root(args.artifact_root, args.lane)
    if root.exists():
        shutil.rmtree(root)
    official = root / "official"
    official.mkdir(parents=True)
    expected_failures = root / "expected-failures.yml"
    expected_failures.write_text("server: []\n", encoding="utf-8")
    metadata = {
        "oracle": PACKAGE,
        "target": args.lane,
        "spec_version": args.spec_version,
        "server_era": args.server_era,
        "suite": "all",
        "fixture": {
            "repository": REPOSITORY,
            "revision": REVISION,
            "server_id": SERVER_ID,
        },
    }
    (root / "metadata.json").write_text(
        json.dumps(metadata, indent=2) + "\n", encoding="utf-8"
    )
    command = [
        "npx",
        "-y",
        PACKAGE,
        "server",
        "--url",
        args.endpoint,
        "--suite",
        "all",
        "--spec-version",
        args.spec_version,
        "--expected-failures",
        str(expected_failures),
        "--output-dir",
        str(official),
        "--verbose",
    ]
    print(f"Conformance lane {args.lane}: client={args.spec_version}, server={args.server_era}")
    with (root / "runner.log").open("wb") as log:
        completed = subprocess.run(
            command,
            stdout=log,
            stderr=subprocess.STDOUT,
            check=False,
            env=npm_environment(),
        )
    if any(official.rglob("checks.json")):
        (root / "complete").write_text("complete\n", encoding="utf-8")
    else:
        print(f"{args.lane}: official runner produced no checks.json files", file=sys.stderr)
        return completed.returncode or 1
    return completed.returncode


def _scenario_name(checks_file: Path) -> str:
    name = checks_file.parent.name
    match = re.match(r"^server-(.+?)-\d{4}-\d{2}-\d{2}T", name)
    return match.group(1) if match else name.removeprefix("server-")


def load_outcomes(root: Path) -> tuple[dict, dict[str, tuple[str, list[tuple[str, str]]]]]:
    metadata = json.loads((root / "metadata.json").read_text(encoding="utf-8"))
    outcomes: dict[str, tuple[str, list[tuple[str, str]]]] = {}
    for checks_file in (root / "official").rglob("checks.json"):
        checks = json.loads(checks_file.read_text(encoding="utf-8"))
        if not isinstance(checks, list) or not checks:
            raise RuntimeError(f"empty or invalid official result: {checks_file}")
        statuses = {str(check.get("status", "")) for check in checks if isinstance(check, dict)}
        if "FAILURE" in statuses:
            outcome = "failure"
        elif statuses & {"SUCCESS", "WARNING"}:
            outcome = "compliant"
        elif statuses == {"SKIPPED"}:
            outcome = "not applicable"
        else:
            outcome = "ambiguous"
        references: list[tuple[str, str]] = []
        for check in checks:
            for reference in check.get("specReferences", []) if isinstance(check, dict) else []:
                if isinstance(reference, dict) and reference.get("url"):
                    references.append((str(reference.get("id", "reference")), str(reference["url"])))
        scenario = _scenario_name(checks_file)
        if scenario in outcomes:
            raise RuntimeError(f"duplicate official conformance scenario: {scenario}")
        outcomes[scenario] = (outcome, references)
    return metadata, outcomes


def _classification(values: list[str]) -> str:
    fixture, controlplane, dataplane = values
    if any(value in {"ambiguous", "missing"} for value in values):
        return "ambiguous"
    if all(value == "not applicable" for value in values):
        return "not applicable"
    failures = [value == "failure" for value in values]
    labels = {
        (False, False, False): "all compliant",
        (True, False, False): "fixture-only failure",
        (False, True, False): "control-plane only failure",
        (False, False, True): "dataplane only failure",
        (True, True, False): "fixture + control-plane failure",
        (True, False, True): "fixture + dataplane failure",
        (False, True, True): "both gateways only failure",
        (True, True, True): "shared failure",
    }
    return labels[tuple(failures)]


def report(artifact_root: Path, output_dir: Path) -> Path:
    loaded = {}
    for lane in LANES:
        root = lane_root(artifact_root, lane)
        if (root / "complete").is_file():
            if (root / "complete").read_bytes() != b"complete\n":
                raise RuntimeError(f"invalid completion marker for {lane}")
            metadata, outcomes = load_outcomes(root)
            if metadata.get("target") != lane:
                raise RuntimeError(f"conformance metadata target does not match {lane}")
            if metadata.get("oracle") != PACKAGE:
                raise RuntimeError(f"conformance artifacts for {lane} use an unknown oracle")
            loaded[lane] = (metadata, outcomes)
    if not loaded:
        raise RuntimeError(f"no complete conformance artifacts below {artifact_root}")
    metadata = next(iter(loaded.values()))[0]
    for candidate, _ in loaded.values():
        for key in ("oracle", "spec_version", "server_era", "suite", "fixture"):
            if candidate.get(key) != metadata.get(key):
                raise RuntimeError("conformance artifacts were produced by incompatible runs")

    scenarios = sorted({name for _, outcomes in loaded.values() for name in outcomes})
    lines = [
        "# MCP Conformance Comparison",
        "",
        f"- Official oracle: `{metadata['oracle']}`",
        f"- Client specification: `{metadata['spec_version']}`",
        f"- Upstream server era: `{metadata['server_era']}`",
        f"- Suite: `{metadata['suite']}`",
        f"- Fixture source: `{metadata['fixture']['repository']}` at `{metadata['fixture']['revision']}`",
        "",
        "## Target outcomes",
        "",
        "| Target | Compliant | Failed | Not applicable | Ambiguous | Missing |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    labels = {"fixture-direct": "Fixture direct", "controlplane": "Control plane", "dataplane": "Dataplane"}
    for lane in LANES:
        outcomes = loaded.get(lane, ({}, {}))[1]
        counts = {value: 0 for value in ("compliant", "failure", "not applicable", "ambiguous")}
        for outcome, _ in outcomes.values():
            counts[outcome] += 1
        lines.append(
            f"| {labels[lane]} | {counts['compliant']} | {counts['failure']} | {counts['not applicable']} | {counts['ambiguous']} | {len(scenarios) - len(outcomes)} |"
        )
    lines += [
        "",
        "## Scenarios",
        "",
        "| Scenario | Fixture direct | Control plane | Dataplane | Classification | Specification references |",
        "|---|---|---|---|---|---|",
    ]
    for scenario in scenarios:
        values = []
        references: dict[tuple[str, str], None] = {}
        for lane in LANES:
            outcome, refs = loaded.get(lane, ({}, {}))[1].get(scenario, ("missing", []))
            values.append(outcome)
            references.update(dict.fromkeys(refs))
        reference_text = "<br>".join(f"[{name}]({url})" for name, url in references) or "-"
        lines.append(
            f"| {scenario} | {values[0]} | {values[1]} | {values[2]} | {_classification(values)} | {reference_text} |"
        )
    output_dir.mkdir(parents=True, exist_ok=True)
    destination = output_dir / "mcp-conformance-comparison.md"
    destination.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return destination


def clear(artifact_root: Path) -> None:
    root = artifact_root / "conformance"
    if root.exists():
        shutil.rmtree(root)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    provision_parser = commands.add_parser("provision")
    provision_parser.add_argument("--base-url", required=True)
    cleanup_parser = commands.add_parser("cleanup")
    cleanup_parser.add_argument("--base-url", required=True)
    cleanup_parser.add_argument("--gateway-id")

    lane_parser = commands.add_parser("run-lane")
    lane_parser.add_argument("--lane", choices=LANES, required=True)
    lane_parser.add_argument("--endpoint", required=True)
    lane_parser.add_argument("--spec-version", required=True)
    lane_parser.add_argument("--server-era", choices=("dual", "legacy", "modern"), required=True)
    lane_parser.add_argument("--artifact-root", type=Path, required=True)

    report_parser = commands.add_parser("report")
    report_parser.add_argument("--artifact-root", type=Path, required=True)
    report_parser.add_argument("--output-dir", type=Path, required=True)
    clear_parser = commands.add_parser("clear")
    clear_parser.add_argument("--artifact-root", type=Path, required=True)
    args = parser.parse_args()

    if args.command in {"provision", "cleanup"}:
        token = os.environ.get("MCPGATEWAY_BEARER_TOKEN", "")
        if not token:
            parser.error("MCPGATEWAY_BEARER_TOKEN is required")
        client = ApiClient(args.base_url, token)
        if args.command == "provision":
            print(provision(client))
        else:
            cleanup(client, args.gateway_id)
    elif args.command == "run-lane":
        raise SystemExit(run_lane(args))
    elif args.command == "report":
        print(report(args.artifact_root, args.output_dir))
    else:
        clear(args.artifact_root)


if __name__ == "__main__":
    main()
