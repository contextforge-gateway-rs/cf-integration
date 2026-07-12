FROM node:22-bookworm-slim

ARG MCP_CONFORMANCE_REVISION=21a9a2febd7100d7c17ac1021ee7f2ed9f66a1e0

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /opt
RUN git clone https://github.com/modelcontextprotocol/conformance.git mcp-conformance \
    && git -C mcp-conformance checkout --detach "${MCP_CONFORMANCE_REVISION}"

WORKDIR /opt/mcp-conformance/examples/servers/typescript
RUN npm ci

COPY docker/patch-mcp-conformance-hosts.mjs /usr/local/bin/patch-mcp-conformance-hosts.mjs
RUN node /usr/local/bin/patch-mcp-conformance-hosts.mjs everything-server.ts

WORKDIR /opt/mcp-conformance
RUN git diff --exit-code -- . ':(exclude)examples/servers/typescript/everything-server.ts' \
    && test "$(git diff --numstat -- examples/servers/typescript/everything-server.ts | awk 'NF == 3 && $1 == 1 && $2 == 1 { count++ } END { print count + 0 }')" = 1 \
    && test "$(grep -Fxc "const app = createMcpExpressApp({ allowedHosts: ['mcp_conformance_server', 'localhost', '127.0.0.1', '::1'] });" examples/servers/typescript/everything-server.ts)" = 1

WORKDIR /opt/mcp-conformance/examples/servers/typescript
ENV PORT=3000
EXPOSE 3000
CMD ["npm", "start"]
