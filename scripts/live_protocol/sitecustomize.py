"""Select the MCP SDK protocol version for cf-integration live tests."""

from __future__ import annotations

import os
import sys


def _select_protocol_version() -> None:
    selected = os.environ.get("CF_LIVE_MCP_PROTOCOL_VERSION")
    if not selected:
        return

    try:
        from mcp import types
        from mcp.shared.version import SUPPORTED_PROTOCOL_VERSIONS
    except ModuleNotFoundError as error:
        if error.name == "mcp" or error.name.startswith("mcp."):
            return
        raise

    if selected not in SUPPORTED_PROTOCOL_VERSIONS:
        supported = ", ".join(SUPPORTED_PROTOCOL_VERSIONS)
        print(
            f"unsupported live MCP protocol version {selected!r}; "
            f"the installed MCP SDK supports: {supported}",
            file=sys.stderr,
            flush=True,
        )
        os._exit(2)

    types.LATEST_PROTOCOL_VERSION = selected


_select_protocol_version()
