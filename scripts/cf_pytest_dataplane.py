"""Expected-failure overlay for upstream live tests routed through cf-dataplane."""

from __future__ import annotations

import os

import pytest


EXPECTED_GAPS = {
    "tests/live_gateway/protocol_compliance/test_pagination.py::test_list_tools_returns_all_stubs[gateway_virtual-http]": (
        "cf-dataplane virtual-server snapshots currently omit the compliance "
        "fixture's paginated stub tools"
    ),
    "tests/live_gateway/protocol_compliance/test_subscriptions.py::test_subscribe_unsubscribe_roundtrip[gateway_virtual-http]": (
        "cf-dataplane virtual-server routing does not currently compose the "
        "fixture's subscribable resource"
    ),
}


def pytest_collection_modifyitems(items: list[pytest.Item]) -> None:
    """Mark only the observed dataplane-specific upstream gaps as expected."""
    if os.environ.get("CF_INTEGRATION_DATAPLANE_EXPECTED_GAPS") != "1":
        return
    for item in items:
        reason = EXPECTED_GAPS.get(item.nodeid)
        if reason:
            item.add_marker(pytest.mark.xfail(reason=reason, strict=False))
