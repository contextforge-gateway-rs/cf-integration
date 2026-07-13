//! MCP transport, gateway, authentication proxy, and probe primitives.

pub mod auth_proxy;
pub mod backend_identity;
pub mod gateway;
pub mod http_transport;
pub mod mcp;
pub mod probe;
mod topology;

pub use topology::GatewayTopology;
