FROM rust:1.96.1 AS builder
WORKDIR /tmp/

RUN <<EOF
apt update
apt install -y git ca-certificates protobuf-compiler
git clone https://github.com/contextforge-gateway-rs/mcp-rust-sdk.git rust-sdk
EOF
WORKDIR /tmp/rust-sdk
RUN git checkout enabling_propagation_of_new_session_id_2
WORKDIR /tmp/rust-sdk/examples/servers

RUN \
    --mount=type=cache,id=cargo,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git \
    cargo fetch
RUN \
    --mount=type=cache,id=cargo,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git \
    cargo build --release --example servers_counter_streamhttp

FROM debian:trixie-slim
COPY --from=builder /tmp/rust-sdk/target/release/examples/servers_counter_streamhttp /servers_counter_streamhttp
LABEL org.opencontainers.image.source=https://github.com/contextforge-gateway-rs/cf-integration
LABEL org.opencontainers.image.description="cf-integration MCP counter"
ENTRYPOINT ["/servers_counter_streamhttp"]
