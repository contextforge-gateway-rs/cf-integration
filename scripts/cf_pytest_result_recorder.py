"""Pytest plugin used by cf-integration.sh to record per-test results."""

from __future__ import annotations

import os


def _result_file() -> str | None:
    return os.environ.get("CF_TEST_RESULT_FILE")


def _status(report) -> str | None:
    was_xfail = getattr(report, "wasxfail", None)
    if report.when == "setup":
        if report.outcome == "failed":
            return "ERROR"
        if report.outcome == "skipped":
            return "SKIP"
        return None

    if report.when == "call":
        if was_xfail:
            if report.outcome == "skipped":
                return "XFAIL"
            if report.outcome == "passed":
                return "XPASS"
        if report.outcome == "passed":
            return "PASS"
        if report.outcome == "failed":
            return "FAIL"
        if report.outcome == "skipped":
            return "SKIP"
        return None

    if report.when == "teardown" and report.outcome == "failed":
        return "ERROR"

    return None


def pytest_runtest_logreport(report):  # noqa: D401
    result_file = _result_file()
    if not result_file:
        return

    status = _status(report)
    if not status:
        return

    with open(result_file, "a", encoding="utf-8") as handle:
        handle.write(f"{status}\t{report.duration:.3f}s\t{report.nodeid}\n")
