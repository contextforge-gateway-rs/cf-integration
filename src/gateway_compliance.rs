//! Gateway-specific live compliance cases not covered by the generic oracle.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, ORIGIN};
use reqwest::{Client, Request, StatusCode};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use cf_integration_platform::StackMode;

use crate::backend_identity::{BACKEND_HEADER, BackendIdentity, sanitized_backend_value};
use crate::gateway::{
    Exchange, GatewayClient, GatewayError, GatewayRequest, HeaderOverride, MCP_PROTOCOL_VERSION,
    MCP_SESSION_ID, RequestCapture,
};
use crate::mcp::{ACCEPT as MCP_ACCEPT, initialize_with_id_and_version, tool_call_args};

/// Outcome of one gateway-specific compliance case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GatewayCaseStatus {
    Passed,
    Failed,
    NotApplicable,
    FixtureFailure,
}

impl GatewayCaseStatus {
    /// Stable report spelling.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::NotApplicable => "not applicable",
            Self::FixtureFailure => "fixture failure",
        }
    }
}

/// Safe HTTP request diagnostic attached to a failed compliance case.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "availability", rename_all = "kebab-case")]
pub enum GatewayRequestEvidence {
    /// The outbound request was captured before it was sent.
    Captured {
        method: String,
        url: String,
        headers: BTreeMap<String, String>,
        body: Option<String>,
    },
    /// No trustworthy request capture was available.
    Unavailable {
        #[serde(rename = "unavailable_reason")]
        reason: String,
    },
}

impl Serialize for GatewayRequestEvidence {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("GatewayRequestEvidence", 6)?;
        match self {
            Self::Captured {
                method,
                url,
                headers,
                body,
            } => {
                state.serialize_field("availability", "captured")?;
                state.serialize_field("method", method)?;
                state.serialize_field("url", url)?;
                state.serialize_field("headers", headers)?;
                state.serialize_field("body", body)?;
                state.serialize_field("unavailable_reason", &Option::<String>::None)?;
            }
            Self::Unavailable { reason } => {
                state.serialize_field("availability", "unavailable")?;
                state.serialize_field("method", &Option::<String>::None)?;
                state.serialize_field("url", &Option::<String>::None)?;
                state.serialize_field("headers", &Option::<BTreeMap<String, String>>::None)?;
                state.serialize_field("body", &Option::<String>::None)?;
                state.serialize_field("unavailable_reason", reason)?;
            }
        }
        state.end()
    }
}

/// Safe HTTP response diagnostic attached to a failed compliance case.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "availability", rename_all = "kebab-case")]
pub enum GatewayResponseEvidence {
    /// The inbound response was captured completely within the safety limit.
    Captured {
        status: u16,
        headers: BTreeMap<String, String>,
        body: String,
    },
    /// The response status and headers were captured, but its body was not.
    /// This is expected for an open SSE stream and on fail-closed backend
    /// identity checks.
    HeadersCaptured {
        status: u16,
        headers: BTreeMap<String, String>,
        #[serde(rename = "unavailable_reason")]
        reason: String,
    },
    /// No trustworthy response capture was available.
    Unavailable {
        #[serde(rename = "unavailable_reason")]
        reason: String,
    },
}

impl Serialize for GatewayResponseEvidence {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("GatewayResponseEvidence", 5)?;
        match self {
            Self::Captured {
                status,
                headers,
                body,
            } => {
                state.serialize_field("availability", "captured")?;
                state.serialize_field("status", status)?;
                state.serialize_field("headers", headers)?;
                state.serialize_field("body", body)?;
                state.serialize_field("unavailable_reason", &Option::<String>::None)?;
            }
            Self::HeadersCaptured {
                status,
                headers,
                reason,
            } => {
                state.serialize_field("availability", "headers-captured")?;
                state.serialize_field("status", status)?;
                state.serialize_field("headers", headers)?;
                state.serialize_field("body", &Option::<String>::None)?;
                state.serialize_field("unavailable_reason", reason)?;
            }
            Self::Unavailable { reason } => {
                state.serialize_field("availability", "unavailable")?;
                state.serialize_field("status", &Option::<u16>::None)?;
                state.serialize_field("headers", &Option::<BTreeMap<String, String>>::None)?;
                state.serialize_field("body", &Option::<String>::None)?;
                state.serialize_field("unavailable_reason", reason)?;
            }
        }
        state.end()
    }
}

/// Structured, redacted diagnostic evidence for a failed compliance case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayFailureEvidence {
    pub stack_mode: String,
    pub protocol_version: String,
    pub request: GatewayRequestEvidence,
    pub response: GatewayResponseEvidence,
}

/// One named gateway-specific case result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayCaseResult {
    pub name: String,
    pub category: String,
    pub status: GatewayCaseStatus,
    pub specification: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_evidence: Option<GatewayFailureEvidence>,
}

/// Results from one stack topology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayComplianceReport {
    pub mode: String,
    pub specification_version: String,
    pub cases: Vec<GatewayCaseResult>,
}

impl GatewayComplianceReport {
    /// Whether every applicable case passed.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.cases.iter().all(|case| {
            matches!(
                case.status,
                GatewayCaseStatus::Passed | GatewayCaseStatus::NotApplicable
            )
        })
    }
}

/// Live gateway-specific suite inputs.
pub struct GatewayComplianceConfig<'a> {
    pub mode: StackMode,
    pub base_url: &'a str,
    pub server_id: &'a str,
    pub bearer_token: &'a str,
    pub wrong_scope_token: Option<&'a str>,
    pub protocol_version: &'a str,
}

/// MCP specification version implemented by the gateway-specific case catalog.
pub const GATEWAY_SPEC_VERSION: &str = "2025-11-25";

/// Runs public-endpoint gateway checks and records every case instead of
/// stopping after the first implementation failure.
///
/// # Errors
///
/// Returns only fixture/client-construction failures that prevent the suite
/// from starting. Protocol differences are returned as failed case rows.
pub async fn run_gateway_compliance(
    config: &GatewayComplianceConfig<'_>,
) -> Result<GatewayComplianceReport> {
    anyhow::ensure!(
        config.protocol_version == GATEWAY_SPEC_VERSION,
        "gateway compliance cases support MCP {GATEWAY_SPEC_VERSION}; requested {}",
        config.protocol_version
    );
    let mut client = GatewayClient::builder(
        config.mode,
        config.base_url,
        config.server_id,
        config.bearer_token,
    )
    .protocol_version(config.protocol_version)
    .build()
    .context("failed to construct gateway compliance client")?;
    let endpoint = client.endpoint().clone();
    let http = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(CASE_TIMEOUT)
        .build()
        .context("failed to construct gateway compliance HTTP client")?;
    let mut cases = Vec::new();

    cases.push(authentication_required(&http, &endpoint, config).await);
    cases.push(invalid_origin(&http, &endpoint, config.bearer_token, config).await);
    cases.push(wrong_scope(config, config.wrong_scope_token).await);

    let initialize = match send_gateway(
        &mut client,
        GatewayRequest::initialize(json!(1)),
        config.mode,
    )
    .await
    {
        Ok(exchange) => exchange,
        Err(error) => {
            cases.push(failed_with_evidence(
                "protocol.initialize",
                "Protocol negotiation",
                LIFECYCLE_SPEC,
                error.detail,
                error.evidence,
            ));
            add_blocked_cases(&mut cases, "initialize failed");
            return Ok(report(config, cases));
        }
    };
    cases.push(passed(
        "protocol.initialize",
        "Protocol negotiation",
        LIFECYCLE_SPEC,
        "initialize returned a valid JSON-RPC response",
    ));
    let initialize_result = result_object(&initialize).cloned();
    cases.extend(validate_initialize_result(
        &initialize,
        config.protocol_version,
    ));
    cases.push(if client.session_id().is_some() {
        passed(
            "session.creation",
            "Session handling",
            TRANSPORT_SPEC,
            "initialize returned a non-empty MCP session header",
        )
    } else {
        not_applicable(
            "session.creation",
            "Session handling",
            TRANSPORT_SPEC,
            "server selected stateless operation and omitted MCP-Session-Id",
        )
    });

    cases.push(
        match send_gateway(&mut client, GatewayRequest::initialized(), config.mode).await {
            Ok(_) => passed(
                "protocol.initialized-notification",
                "Protocol negotiation",
                LIFECYCLE_SPEC,
                "initialized notification returned HTTP 202 with no body",
            ),
            Err(error) => failed_with_evidence(
                "protocol.initialized-notification",
                "Protocol negotiation",
                LIFECYCLE_SPEC,
                error.detail,
                error.evidence,
            ),
        },
    );
    cases.extend(ping_cases(&mut client, config.mode).await);

    let tools_exchange = send_gateway(
        &mut client,
        GatewayRequest::request("tools/list", Some(json!({})), json!(2)),
        config.mode,
    )
    .await;
    let (tools, tools_evidence) = match tools_exchange {
        Ok(exchange) => {
            let evidence = evidence_from_exchange(&exchange, config.protocol_version);
            let catalog = validate_named_catalog(&exchange, "tools", "inputSchema");
            match catalog.error {
                None => {
                    cases.push(passed(
                        "preservation.tools-list",
                        "Gateway preservation",
                        TOOLS_SPEC,
                        format!("validated {} tool definitions", catalog.entries.len()),
                    ));
                    (catalog.entries, Some(evidence))
                }
                Some(detail) => {
                    cases.push(failed_with_evidence(
                        "preservation.tools-list",
                        "Gateway preservation",
                        TOOLS_SPEC,
                        detail,
                        evidence.clone(),
                    ));
                    (catalog.entries, Some(evidence))
                }
            }
        }
        Err(error) => {
            let evidence = error.evidence;
            cases.push(failed_with_evidence(
                "preservation.tools-list",
                "Gateway preservation",
                TOOLS_SPEC,
                error.detail,
                evidence.clone(),
            ));
            (Vec::new(), Some(evidence))
        }
    };

    cases.push(
        compare_second_tools_list(&mut client, &tools, tools_evidence.as_ref(), config.mode).await,
    );
    cases.push(call_safe_tool(&mut client, &tools, config.mode).await);
    cases.extend(capability_cases(&mut client, initialize_result.as_ref(), config.mode).await);
    cases.extend(
        transport_negative_cases(
            &mut client,
            &http,
            &endpoint,
            config.bearer_token,
            config.protocol_version,
            config.mode,
        )
        .await,
    );
    cases.extend(virtualization_cases(
        &tools,
        tools_evidence.as_ref(),
        config,
    ));
    cases.extend(federation_and_security_gaps(
        config.mode,
        &tools,
        tools_evidence.as_ref(),
        client.session_id().is_some(),
        config.protocol_version,
    ));
    cases.extend(delete_and_reuse_session(&mut client, config.mode).await);

    Ok(report(config, cases))
}

const LIFECYCLE_SPEC: &str =
    "https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle";
const TRANSPORT_SPEC: &str =
    "https://modelcontextprotocol.io/specification/2025-11-25/basic/transports";
const SECURITY_SPEC: &str =
    "https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning";
const AUTHORIZATION_ERROR_SPEC: &str =
    "https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization#error-handling";
const TOKEN_HANDLING_SPEC: &str =
    "https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization#token-handling";
const TOOLS_SPEC: &str = "https://modelcontextprotocol.io/specification/2025-11-25/server/tools";
const RESOURCES_SPEC: &str =
    "https://modelcontextprotocol.io/specification/2025-11-25/server/resources";
const PROMPTS_SPEC: &str =
    "https://modelcontextprotocol.io/specification/2025-11-25/server/prompts";
const PING_SPEC: &str =
    "https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/ping";
const CASE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_DIAGNOSTIC_BODY_BYTES: usize = 1024 * 1024;

struct RawExchange {
    status: StatusCode,
    content_type: Option<String>,
    evidence: GatewayFailureEvidence,
}

struct RawExchangeFailure {
    detail: String,
    evidence: GatewayFailureEvidence,
    kind: RawExchangeFailureKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RawExchangeFailureKind {
    Fixture,
    Compliance,
}

struct GatewaySendFailure {
    detail: String,
    evidence: GatewayFailureEvidence,
}

enum ResponseBodyCapture {
    Bounded,
    HeadersOnly(&'static str),
}

async fn send_raw(
    client: &Client,
    request: Request,
    mode: StackMode,
    protocol_version: &str,
    secrets: &[&str],
) -> std::result::Result<RawExchange, RawExchangeFailure> {
    send_raw_with_capture(
        client,
        request,
        mode,
        protocol_version,
        secrets,
        ResponseBodyCapture::Bounded,
    )
    .await
}

async fn send_raw_headers_only(
    client: &Client,
    request: Request,
    mode: StackMode,
    protocol_version: &str,
    secrets: &[&str],
) -> std::result::Result<RawExchange, RawExchangeFailure> {
    send_raw_with_capture(
        client,
        request,
        mode,
        protocol_version,
        secrets,
        ResponseBodyCapture::HeadersOnly(
            "response body was intentionally not consumed because an SSE stream may remain open",
        ),
    )
    .await
}

async fn send_raw_with_capture(
    client: &Client,
    request: Request,
    mode: StackMode,
    protocol_version: &str,
    secrets: &[&str],
    body_capture: ResponseBodyCapture,
) -> std::result::Result<RawExchange, RawExchangeFailure> {
    let request_evidence = capture_raw_request(&request, secrets);
    let mut response = client
        .execute(request)
        .await
        .map_err(|_| RawExchangeFailure {
            detail: "request could not be completed".to_owned(),
            kind: RawExchangeFailureKind::Fixture,
            evidence: GatewayFailureEvidence {
                stack_mode: mode_label(mode).to_owned(),
                protocol_version: protocol_version.to_owned(),
                request: request_evidence.clone(),
                response: GatewayResponseEvidence::Unavailable {
                    reason: "the endpoint returned no HTTP response".to_owned(),
                },
            },
        })?;
    let status = response.status();
    let raw_headers = response.headers().clone();
    let content_type = raw_headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(normalize_content_type);
    let mut response_secrets = secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .map(|secret| (*secret).to_owned())
        .collect::<Vec<_>>();
    response_secrets.extend(sensitive_values(&raw_headers));
    let response_secret_refs = response_secrets
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let captured_headers = capture_raw_headers(&raw_headers, &response_secret_refs);
    if mode == StackMode::Dataplane
        && let Some(detail) = BackendIdentity::from_headers(&raw_headers).dataplane_error()
    {
        return Err(RawExchangeFailure {
            detail: detail.to_owned(),
            kind: RawExchangeFailureKind::Compliance,
            evidence: GatewayFailureEvidence {
                stack_mode: mode_label(mode).to_owned(),
                protocol_version: protocol_version.to_owned(),
                request: request_evidence,
                response: GatewayResponseEvidence::HeadersCaptured {
                    status: status.as_u16(),
                    headers: captured_headers,
                    reason: "response body was not read because backend identity validation failed"
                        .to_owned(),
                },
            },
        });
    }
    if let ResponseBodyCapture::HeadersOnly(reason) = body_capture {
        return Ok(RawExchange {
            status,
            content_type,
            evidence: GatewayFailureEvidence {
                stack_mode: mode_label(mode).to_owned(),
                protocol_version: protocol_version.to_owned(),
                request: request_evidence,
                response: GatewayResponseEvidence::HeadersCaptured {
                    status: status.as_u16(),
                    headers: captured_headers,
                    reason: reason.to_owned(),
                },
            },
        });
    }
    let body = read_bounded_body(&mut response)
        .await
        .map_err(|detail| RawExchangeFailure {
            detail: detail.clone(),
            kind: RawExchangeFailureKind::Compliance,
            evidence: GatewayFailureEvidence {
                stack_mode: mode_label(mode).to_owned(),
                protocol_version: protocol_version.to_owned(),
                request: request_evidence.clone(),
                response: GatewayResponseEvidence::HeadersCaptured {
                    status: status.as_u16(),
                    headers: captured_headers.clone(),
                    reason: detail,
                },
            },
        })?;
    let evidence = GatewayFailureEvidence {
        stack_mode: mode_label(mode).to_owned(),
        protocol_version: protocol_version.to_owned(),
        request: request_evidence,
        response: GatewayResponseEvidence::Captured {
            status: status.as_u16(),
            headers: captured_headers,
            body: sanitize_and_redact(&String::from_utf8_lossy(&body), &response_secret_refs),
        },
    };
    Ok(RawExchange {
        status,
        content_type,
        evidence,
    })
}

fn capture_raw_request(request: &Request, secrets: &[&str]) -> GatewayRequestEvidence {
    let body = match request.body() {
        Some(body) => match body.as_bytes() {
            Some(body) => Some(sanitize_and_redact(&String::from_utf8_lossy(body), secrets)),
            None => {
                return GatewayRequestEvidence::Unavailable {
                    reason: "request body was streaming and could not be captured".to_owned(),
                };
            }
        },
        None => None,
    };
    GatewayRequestEvidence::Captured {
        method: request.method().to_string(),
        url: sanitize_and_redact(request.url().as_str(), secrets),
        headers: capture_raw_headers(request.headers(), secrets),
        body,
    }
}

fn capture_raw_headers(headers: &HeaderMap, secrets: &[&str]) -> BTreeMap<String, String> {
    let backend_identity = BackendIdentity::from_headers(headers);
    headers
        .iter()
        .map(|(name, value)| {
            let value = if value.is_sensitive() || is_sensitive_header(name.as_str()) {
                "<redacted>".to_owned()
            } else if name.as_str().eq_ignore_ascii_case(BACKEND_HEADER) {
                if matches!(
                    backend_identity,
                    BackendIdentity::Invalid | BackendIdentity::Multiple
                ) {
                    "<invalid>".to_owned()
                } else {
                    sanitized_backend_value(value).to_owned()
                }
            } else {
                value.to_str().map_or_else(
                    |_| "<non-UTF-8 header value>".to_owned(),
                    |value| sanitize_and_redact(value, secrets),
                )
            };
            (name.as_str().to_ascii_lowercase(), value)
        })
        .collect()
}

fn sensitive_values(headers: &HeaderMap) -> Vec<String> {
    headers
        .iter()
        .filter(|(name, value)| value.is_sensitive() || is_sensitive_header(name.as_str()))
        .filter_map(|(_, value)| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_sensitive_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | MCP_SESSION_ID
    )
}

async fn read_bounded_body(
    response: &mut reqwest::Response,
) -> std::result::Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "response body could not be read".to_owned())?
    {
        if body.len().saturating_add(chunk.len()) > MAX_DIAGNOSTIC_BODY_BYTES {
            return Err(format!(
                "response body exceeded the {MAX_DIAGNOSTIC_BODY_BYTES}-byte diagnostic limit"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn normalize_content_type(value: &str) -> String {
    value
        .split_once(';')
        .map_or(value, |(media_type, _)| media_type)
        .trim()
        .to_owned()
}

fn sanitize_and_redact(value: &str, secrets: &[&str]) -> String {
    let mut redacted = value.to_owned();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        redacted = redacted.replace(secret, "<redacted>");
    }
    sanitize(&redacted)
}

fn evidence_from_exchange(exchange: &Exchange, protocol_version: &str) -> GatewayFailureEvidence {
    GatewayFailureEvidence {
        stack_mode: mode_label(exchange.mode()).to_owned(),
        protocol_version: protocol_version.to_owned(),
        request: GatewayRequestEvidence::Captured {
            method: exchange.request().method().to_owned(),
            url: exchange.request().url().to_owned(),
            headers: exchange.request().headers().clone(),
            body: exchange.request().body().map(str::to_owned),
        },
        response: GatewayResponseEvidence::Captured {
            status: exchange.status(),
            headers: exchange.headers().clone(),
            body: exchange.body().to_owned(),
        },
    }
}

fn evidence_from_request(
    request: &RequestCapture,
    protocol_version: &str,
    response_reason: impl Into<String>,
) -> GatewayFailureEvidence {
    GatewayFailureEvidence {
        stack_mode: mode_label(request.mode()).to_owned(),
        protocol_version: protocol_version.to_owned(),
        request: GatewayRequestEvidence::Captured {
            method: request.method().to_owned(),
            url: request.url().to_owned(),
            headers: request.headers().clone(),
            body: request.body().map(str::to_owned),
        },
        response: GatewayResponseEvidence::Unavailable {
            reason: sanitize(&response_reason.into()),
        },
    }
}

fn unavailable_evidence(
    mode: StackMode,
    protocol_version: &str,
    reason: impl Into<String>,
) -> GatewayFailureEvidence {
    let reason = sanitize(&reason.into());
    GatewayFailureEvidence {
        stack_mode: mode_label(mode).to_owned(),
        protocol_version: protocol_version.to_owned(),
        request: GatewayRequestEvidence::Unavailable {
            reason: reason.clone(),
        },
        response: GatewayResponseEvidence::Unavailable { reason },
    }
}

fn evidence_from_gateway_error(
    error: &GatewayError,
    protocol_version: &str,
) -> GatewayFailureEvidence {
    match error {
        GatewayError::Exchange { exchange, .. } => {
            evidence_from_exchange(exchange, protocol_version)
        }
        GatewayError::Request { request, .. } => evidence_from_request(
            request,
            protocol_version,
            "the request failed before an HTTP response was received",
        ),
        GatewayError::Configuration { mode, .. } => unavailable_evidence(
            *mode,
            protocol_version,
            "request construction failed before a trustworthy HTTP request was available",
        ),
    }
}

async fn send_gateway(
    client: &mut GatewayClient,
    request: GatewayRequest,
    mode: StackMode,
) -> std::result::Result<Exchange, GatewaySendFailure> {
    let protocol_version = client.protocol_version().to_owned();
    match tokio::time::timeout(CASE_TIMEOUT, client.send(request)).await {
        Ok(result) => result.map_err(|error| GatewaySendFailure {
            detail: error.to_string(),
            evidence: evidence_from_gateway_error(&error, &protocol_version),
        }),
        Err(_) => {
            let detail = format!(
                "gateway request timed out after {} seconds",
                CASE_TIMEOUT.as_secs()
            );
            Err(GatewaySendFailure {
                evidence: unavailable_evidence(mode, &protocol_version, &detail),
                detail,
            })
        }
    }
}

fn report(
    config: &GatewayComplianceConfig<'_>,
    mut cases: Vec<GatewayCaseResult>,
) -> GatewayComplianceReport {
    for case in &mut cases {
        if case.status == GatewayCaseStatus::Failed && case.failure_evidence.is_none() {
            case.failure_evidence = Some(unavailable_evidence(
                config.mode,
                config.protocol_version,
                format!(
                    "case {} failed after local validation, but no specific HTTP exchange was retained",
                    case.name
                ),
            ));
        }
    }
    GatewayComplianceReport {
        mode: mode_label(config.mode).to_owned(),
        specification_version: config.protocol_version.to_owned(),
        cases,
    }
}

async fn authentication_required(
    client: &Client,
    endpoint: &url::Url,
    config: &GatewayComplianceConfig<'_>,
) -> GatewayCaseResult {
    let payload = initialize_with_id_and_version(json!(90), config.protocol_version);
    let request = client
        .post(endpoint.clone())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, MCP_ACCEPT)
        .body(payload.to_string())
        .build();
    let Ok(request) = request else {
        return fixture(
            "security.authentication-required",
            "Security",
            AUTHORIZATION_ERROR_SPEC,
            "could not construct unauthenticated initialize request",
        );
    };
    match send_raw(
        client,
        request,
        config.mode,
        config.protocol_version,
        &[config.bearer_token],
    )
    .await
    {
        Ok(exchange) if exchange.status == StatusCode::UNAUTHORIZED => passed(
            "security.authentication-required",
            "Security",
            AUTHORIZATION_ERROR_SPEC,
            "unauthenticated initialize returned HTTP 401",
        ),
        Ok(exchange) => failed_with_evidence(
            "security.authentication-required",
            "Security",
            AUTHORIZATION_ERROR_SPEC,
            format!("expected HTTP 401, got {}", exchange.status.as_u16()),
            exchange.evidence,
        ),
        Err(failure) if failure.kind == RawExchangeFailureKind::Fixture => fixture(
            "security.authentication-required",
            "Security",
            AUTHORIZATION_ERROR_SPEC,
            "could not reach public endpoint",
        ),
        Err(failure) => failed_with_evidence(
            "security.authentication-required",
            "Security",
            AUTHORIZATION_ERROR_SPEC,
            failure.detail,
            failure.evidence,
        ),
    }
}

async fn invalid_origin(
    client: &Client,
    endpoint: &url::Url,
    token: &str,
    config: &GatewayComplianceConfig<'_>,
) -> GatewayCaseResult {
    let payload = initialize_with_id_and_version(json!(91), config.protocol_version);
    let authorization = HeaderValue::from_str(&format!("Bearer {token}"));
    let Ok(mut authorization) = authorization else {
        return fixture(
            "security.invalid-origin",
            "Security",
            SECURITY_SPEC,
            "fixture bearer token cannot be encoded as an HTTP header",
        );
    };
    authorization.set_sensitive(true);
    let request = client
        .post(endpoint.clone())
        .header(AUTHORIZATION, authorization)
        .header(ORIGIN, "https://attacker.invalid")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, MCP_ACCEPT)
        .body(payload.to_string())
        .build();
    let Ok(request) = request else {
        return fixture(
            "security.invalid-origin",
            "Security",
            SECURITY_SPEC,
            "could not construct invalid-Origin request",
        );
    };
    match send_raw(
        client,
        request,
        config.mode,
        config.protocol_version,
        &[token],
    )
    .await
    {
        Ok(exchange) if exchange.status == StatusCode::FORBIDDEN => passed(
            "security.invalid-origin",
            "Security",
            SECURITY_SPEC,
            "invalid Origin returned HTTP 403",
        ),
        Ok(exchange) => failed_with_evidence(
            "security.invalid-origin",
            "Security",
            SECURITY_SPEC,
            format!("expected HTTP 403, got {}", exchange.status.as_u16()),
            exchange.evidence,
        ),
        Err(failure) if failure.kind == RawExchangeFailureKind::Fixture => fixture(
            "security.invalid-origin",
            "Security",
            SECURITY_SPEC,
            "could not reach public endpoint",
        ),
        Err(failure) => failed_with_evidence(
            "security.invalid-origin",
            "Security",
            SECURITY_SPEC,
            failure.detail,
            failure.evidence,
        ),
    }
}

async fn wrong_scope(
    config: &GatewayComplianceConfig<'_>,
    token: Option<&str>,
) -> GatewayCaseResult {
    if config.mode == StackMode::Controlplane {
        return not_applicable(
            "security.authorization-wrong-server",
            "Security",
            TOKEN_HANDLING_SPEC,
            "raw control-plane /mcp is not scoped by a virtual server path",
        );
    }
    let Some(token) = token else {
        return fixture(
            "security.authorization-wrong-server",
            "Security",
            TOKEN_HANDLING_SPEC,
            "wrong-scope token fixture is unavailable",
        );
    };
    let client = GatewayClient::builder(config.mode, config.base_url, config.server_id, token)
        .protocol_version(config.protocol_version)
        .build();
    let Ok(mut client) = client else {
        return fixture(
            "security.authorization-wrong-server",
            "Security",
            TOKEN_HANDLING_SPEC,
            "wrong-scope token fixture is invalid",
        );
    };
    match send_gateway(
        &mut client,
        GatewayRequest::initialize(json!(92)).unchecked(),
        config.mode,
    )
    .await
    {
        Ok(exchange) if matches!(exchange.status(), 401 | 403 | 404) => passed(
            "security.authorization-wrong-server",
            "Security",
            TOKEN_HANDLING_SPEC,
            format!(
                "wrong-server token was rejected with HTTP {}",
                exchange.status()
            ),
        ),
        Ok(exchange) => {
            let evidence = evidence_from_exchange(&exchange, config.protocol_version);
            failed_with_evidence(
                "security.authorization-wrong-server",
                "Security",
                TOKEN_HANDLING_SPEC,
                format!("wrong-server token returned HTTP {}", exchange.status()),
                evidence,
            )
        }
        Err(failure) if failure.detail.contains("timed out") => fixture(
            "security.authorization-wrong-server",
            "Security",
            TOKEN_HANDLING_SPEC,
            "wrong-scope request timed out",
        ),
        Err(_) => fixture(
            "security.authorization-wrong-server",
            "Security",
            TOKEN_HANDLING_SPEC,
            "wrong-scope request could not be completed",
        ),
    }
}

fn validate_initialize_result(
    exchange: &Exchange,
    protocol_version: &str,
) -> Vec<GatewayCaseResult> {
    let result = result_object(exchange);
    let Some(result) = result else {
        return [
            "protocol.initialize-result",
            "protocol.version-negotiation",
            "protocol.capability-negotiation",
            "protocol.server-info",
        ]
        .into_iter()
        .map(|name| {
            failed_with_evidence(
                name,
                "Protocol negotiation",
                LIFECYCLE_SPEC,
                "initialize result is not an object",
                evidence_from_exchange(exchange, protocol_version),
            )
        })
        .collect();
    };
    vec![
        passed(
            "protocol.initialize-result",
            "Protocol negotiation",
            LIFECYCLE_SPEC,
            "initialize returned an object result",
        ),
        if result.get("protocolVersion").and_then(Value::as_str) == Some(protocol_version) {
            passed(
                "protocol.version-negotiation",
                "Protocol negotiation",
                LIFECYCLE_SPEC,
                "server selected the requested protocol version",
            )
        } else {
            failed_with_evidence(
                "protocol.version-negotiation",
                "Protocol negotiation",
                LIFECYCLE_SPEC,
                "server did not select the requested protocol version",
                evidence_from_exchange(exchange, protocol_version),
            )
        },
        if result
            .get("capabilities")
            .and_then(Value::as_object)
            .and_then(|capabilities| capabilities.get("tools"))
            .and_then(Value::as_object)
            .is_some_and(|tools| tools.get("listChanged").is_none_or(Value::is_boolean))
        {
            passed(
                "protocol.capability-negotiation",
                "Protocol negotiation",
                LIFECYCLE_SPEC,
                "initialize advertised the tools capability required by the live fixture",
            )
        } else {
            failed_with_evidence(
                "protocol.capability-negotiation",
                "Protocol negotiation",
                LIFECYCLE_SPEC,
                "initialize omitted or malformed the tools capability required by the live fixture",
                evidence_from_exchange(exchange, protocol_version),
            )
        },
        if result
            .get("serverInfo")
            .and_then(Value::as_object)
            .is_some_and(|server_info| {
                ["name", "version"].into_iter().all(|field| {
                    server_info
                        .get(field)
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.is_empty())
                })
            })
        {
            passed(
                "protocol.server-info",
                "Protocol negotiation",
                LIFECYCLE_SPEC,
                "initialize advertised serverInfo with non-empty name and version",
            )
        } else {
            failed_with_evidence(
                "protocol.server-info",
                "Protocol negotiation",
                LIFECYCLE_SPEC,
                "initialize omitted or malformed serverInfo name/version",
                evidence_from_exchange(exchange, protocol_version),
            )
        },
    ]
}

fn result_object(exchange: &Exchange) -> Option<&serde_json::Map<String, Value>> {
    exchange
        .message()
        .and_then(|message| message.get("result"))
        .and_then(Value::as_object)
}

async fn ping_cases(client: &mut GatewayClient, mode: StackMode) -> Vec<GatewayCaseResult> {
    let session_assigned = client.session_id().is_some();
    let ping = send_gateway(
        client,
        GatewayRequest::request("ping", None, json!(10)),
        mode,
    )
    .await;
    let ping_case = match ping {
        Ok(exchange) if result_object(&exchange).is_some_and(serde_json::Map::is_empty) => passed(
            "protocol.ping",
            "Utilities",
            PING_SPEC,
            "ping returned an empty object result",
        ),
        Ok(exchange) => {
            let evidence = evidence_from_exchange(&exchange, client.protocol_version());
            failed_with_evidence(
                "protocol.ping",
                "Utilities",
                PING_SPEC,
                "ping did not return an empty object result",
                evidence,
            )
        }
        Err(failure) => failed_with_evidence(
            "protocol.ping",
            "Utilities",
            PING_SPEC,
            failure.detail,
            failure.evidence,
        ),
    };
    let reuse_case = if !session_assigned {
        not_applicable(
            "session.reuse",
            "Session handling",
            TRANSPORT_SPEC,
            "server selected stateless operation and omitted MCP-Session-Id",
        )
    } else if ping_case.status == GatewayCaseStatus::Passed {
        passed(
            "session.reuse",
            "Session handling",
            TRANSPORT_SPEC,
            "post-initialize ping reused the assigned session",
        )
    } else {
        failed_with_evidence(
            "session.reuse",
            "Session handling",
            TRANSPORT_SPEC,
            "post-initialize ping did not complete with the assigned session",
            ping_case.failure_evidence.clone().unwrap_or_else(|| {
                unavailable_evidence(
                    mode,
                    client.protocol_version(),
                    "the preceding ping failure did not retain diagnostic evidence",
                )
            }),
        )
    };
    vec![ping_case, reuse_case]
}

struct NamedCatalogValidation {
    entries: Vec<Value>,
    error: Option<String>,
}

fn validate_named_catalog(
    exchange: &Exchange,
    field: &str,
    schema_field: &str,
) -> NamedCatalogValidation {
    let Some(entries) = result_object(exchange)
        .and_then(|result| result.get(field))
        .and_then(Value::as_array)
    else {
        return NamedCatalogValidation {
            entries: Vec::new(),
            error: Some(format!("{field}/list result omitted its {field} array")),
        };
    };
    let entries = entries.clone();
    if entries.is_empty() {
        return NamedCatalogValidation {
            entries,
            error: Some(format!("{field}/list returned an empty {field} array")),
        };
    }
    let mut names = BTreeSet::new();
    for entry in &entries {
        let Some(object) = entry.as_object() else {
            return NamedCatalogValidation {
                entries,
                error: Some(format!("{field}/list contains a non-object entry")),
            };
        };
        let Some(name) = object
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            return NamedCatalogValidation {
                entries,
                error: Some(format!("{field}/list contains an entry without a name")),
            };
        };
        if !names.insert(name.to_owned()) {
            let error = format!("{field}/list contains duplicate exposed name {name:?}");
            return NamedCatalogValidation {
                entries,
                error: Some(error),
            };
        }
        if object
            .get(schema_field)
            .and_then(Value::as_object)
            .is_none()
        {
            let error = format!("{name:?} omitted object {schema_field}");
            return NamedCatalogValidation {
                entries,
                error: Some(error),
            };
        }
    }
    NamedCatalogValidation {
        entries,
        error: None,
    }
}

async fn compare_second_tools_list(
    client: &mut GatewayClient,
    first: &[Value],
    first_evidence: Option<&GatewayFailureEvidence>,
    mode: StackMode,
) -> GatewayCaseResult {
    if first.is_empty() {
        return failed_with_evidence(
            "preservation.tool-schema-stability",
            "Gateway preservation",
            TOOLS_SPEC,
            "first tools/list result was unavailable",
            first_evidence.cloned().unwrap_or_else(|| {
                unavailable_evidence(
                    mode,
                    client.protocol_version(),
                    "the comparison request was not issued because the first tools/list result was unavailable",
                )
            }),
        );
    }
    let second = match send_gateway(
        client,
        GatewayRequest::request("tools/list", Some(json!({})), json!(3)),
        mode,
    )
    .await
    {
        Ok(exchange) => exchange,
        Err(failure) => {
            return failed_with_evidence(
                "preservation.tool-schema-stability",
                "Gateway preservation",
                TOOLS_SPEC,
                failure.detail,
                failure.evidence,
            );
        }
    };
    let evidence = evidence_from_exchange(&second, client.protocol_version());
    let second_catalog = validate_named_catalog(&second, "tools", "inputSchema");
    if let Some(detail) = second_catalog.error {
        return failed_with_evidence(
            "preservation.tool-schema-stability",
            "Gateway preservation",
            TOOLS_SPEC,
            detail,
            evidence,
        );
    }
    let second_tools = second_catalog.entries;
    if second_tools != first {
        return failed_with_evidence(
            "preservation.tool-schema-stability",
            "Gateway preservation",
            TOOLS_SPEC,
            "repeated tools/list changed names or schemas",
            evidence,
        );
    }
    passed(
        "preservation.tool-schema-stability",
        "Gateway preservation",
        TOOLS_SPEC,
        "repeated tools/list preserved exact tool definitions",
    )
}

async fn call_safe_tool(
    client: &mut GatewayClient,
    tools: &[Value],
    mode: StackMode,
) -> GatewayCaseResult {
    if tools.is_empty() {
        return fixture(
            "preservation.tool-result",
            "Gateway preservation",
            TOOLS_SPEC,
            "tools/list result was unavailable",
        );
    }
    let callable = tools.iter().find_map(|tool| {
        let name = tool.get("name")?.as_str()?;
        tool_call_args(name).map(|arguments| (name, arguments))
    });
    let Some((name, arguments)) = callable else {
        return not_applicable(
            "preservation.tool-result",
            "Gateway preservation",
            TOOLS_SPEC,
            "fixture advertises no explicitly safe echo or system-time tool",
        );
    };
    match send_gateway(
        client,
        GatewayRequest::request(
            "tools/call",
            Some(json!({"name": name, "arguments": arguments})),
            json!(4),
        ),
        mode,
    )
    .await
    {
        Ok(exchange) => {
            let valid = result_object(&exchange).is_some_and(|result| {
                result.get("content").and_then(Value::as_array).is_some()
                    && result
                        .get("isError")
                        .is_none_or(|value| value == &Value::Bool(false))
            });
            if valid {
                passed(
                    "preservation.tool-result",
                    "Gateway preservation",
                    TOOLS_SPEC,
                    format!("tool result shape preserved for {name:?}"),
                )
            } else {
                let evidence = evidence_from_exchange(&exchange, client.protocol_version());
                failed_with_evidence(
                    "preservation.tool-result",
                    "Gateway preservation",
                    TOOLS_SPEC,
                    "tools/call returned a malformed result",
                    evidence,
                )
            }
        }
        Err(error) => failed_with_evidence(
            "preservation.tool-result",
            "Gateway preservation",
            TOOLS_SPEC,
            error.detail,
            error.evidence,
        ),
    }
}

async fn capability_cases(
    client: &mut GatewayClient,
    initialize: Option<&serde_json::Map<String, Value>>,
    mode: StackMode,
) -> Vec<GatewayCaseResult> {
    if initialize.is_none() {
        return [("resources", RESOURCES_SPEC), ("prompts", PROMPTS_SPEC)]
            .into_iter()
            .map(|(field, spec)| {
                fixture(
                    format!("federation.{field}-aggregation"),
                    "Federation",
                    spec,
                    "initialize result was unavailable",
                )
            })
            .collect();
    }
    let capabilities = initialize
        .and_then(|result| result.get("capabilities"))
        .and_then(Value::as_object);
    let mut cases = Vec::new();
    for (capability, method, field, spec, id) in [
        (
            "resources",
            "resources/list",
            "resources",
            RESOURCES_SPEC,
            5_u64,
        ),
        ("prompts", "prompts/list", "prompts", PROMPTS_SPEC, 6_u64),
    ] {
        if !capabilities.is_some_and(|capabilities| capabilities.contains_key(capability)) {
            cases.push(not_applicable(
                format!("federation.{field}-aggregation"),
                "Federation",
                spec,
                format!("server did not advertise the {capability} capability"),
            ));
            continue;
        }
        let case = match send_gateway(
            client,
            GatewayRequest::request(method, Some(json!({})), json!(id)),
            mode,
        )
        .await
        {
            Ok(exchange)
                if result_object(&exchange)
                    .and_then(|result| result.get(field))
                    .and_then(Value::as_array)
                    .is_some() =>
            {
                passed(
                    format!("federation.{field}-aggregation"),
                    "Federation",
                    spec,
                    format!("advertised {capability} capability returned a {field} array"),
                )
            }
            Ok(exchange) => {
                let evidence = evidence_from_exchange(&exchange, client.protocol_version());
                failed_with_evidence(
                    format!("federation.{field}-aggregation"),
                    "Federation",
                    spec,
                    format!("advertised {capability} capability omitted the {field} array"),
                    evidence,
                )
            }
            Err(failure) => failed_with_evidence(
                format!("federation.{field}-aggregation"),
                "Federation",
                spec,
                failure.detail,
                failure.evidence,
            ),
        };
        cases.push(case);
    }
    cases
}

async fn transport_negative_cases(
    client: &mut GatewayClient,
    http: &Client,
    endpoint: &url::Url,
    token: &str,
    protocol_version: &str,
    mode: StackMode,
) -> Vec<GatewayCaseResult> {
    let mut cases = Vec::new();
    cases.push(
        match send_gateway(
            client,
            GatewayRequest::request("ping", None, json!(20))
                .protocol_version(HeaderOverride::Value("unsupported-version".to_owned()))
                .unchecked(),
            mode,
        )
        .await
        {
            Ok(exchange) if exchange.status() == 400 => passed(
                "transport.invalid-protocol-version",
                "HTTP transport",
                TRANSPORT_SPEC,
                "invalid MCP-Protocol-Version returned HTTP 400",
            ),
            Ok(exchange) => {
                let evidence = evidence_from_exchange(&exchange, protocol_version);
                failed_with_evidence(
                    "transport.invalid-protocol-version",
                    "HTTP transport",
                    TRANSPORT_SPEC,
                    format!("expected HTTP 400, got {}", exchange.status()),
                    evidence,
                )
            }
            Err(failure) => failed_with_evidence(
                "transport.invalid-protocol-version",
                "HTTP transport",
                TRANSPORT_SPEC,
                failure.detail,
                failure.evidence,
            ),
        },
    );
    cases.push(if client.session_id().is_none() {
        not_applicable(
            "session.invalid-session",
            "Session handling",
            TRANSPORT_SPEC,
            "server selected stateless operation and does not use session IDs",
        )
    } else {
        match send_gateway(
            client,
            GatewayRequest::request("ping", None, json!(21))
                .session(HeaderOverride::Value("invalid-session".to_owned()))
                .unchecked(),
            mode,
        )
        .await
        {
            Ok(exchange) if exchange.status() == 404 => passed(
                "session.invalid-session",
                "Session handling",
                TRANSPORT_SPEC,
                "invalid session returned HTTP 404",
            ),
            Ok(exchange) => {
                let evidence = evidence_from_exchange(&exchange, protocol_version);
                failed_with_evidence(
                    "session.invalid-session",
                    "Session handling",
                    TRANSPORT_SPEC,
                    format!("expected HTTP 404, got {}", exchange.status()),
                    evidence,
                )
            }
            Err(failure) => failed_with_evidence(
                "session.invalid-session",
                "Session handling",
                TRANSPORT_SPEC,
                failure.detail,
                failure.evidence,
            ),
        }
    });
    cases.push(
        match get_response(
            http,
            endpoint,
            token,
            protocol_version,
            client.session_id(),
            mode,
        )
        .await
        {
            Ok(exchange)
                if exchange.status == StatusCode::OK
                    && exchange
                        .content_type
                        .as_deref()
                        .is_some_and(|value| value.eq_ignore_ascii_case("text/event-stream")) =>
            {
                passed(
                    "transport.get-behaviour",
                    "HTTP transport",
                    TRANSPORT_SPEC,
                    "GET returned an HTTP 200 SSE stream",
                )
            }
            Ok(exchange) if exchange.status == StatusCode::OK => failed_with_evidence(
                "transport.get-behaviour",
                "HTTP transport",
                TRANSPORT_SPEC,
                "GET returned HTTP 200 without text/event-stream content",
                exchange.evidence,
            ),
            Ok(exchange) if exchange.status == StatusCode::METHOD_NOT_ALLOWED => passed(
                "transport.get-behaviour",
                "HTTP transport",
                TRANSPORT_SPEC,
                "GET returned permitted HTTP 405",
            ),
            Ok(exchange) => failed_with_evidence(
                "transport.get-behaviour",
                "HTTP transport",
                TRANSPORT_SPEC,
                format!("GET returned unexpected HTTP {}", exchange.status.as_u16()),
                exchange.evidence,
            ),
            Err(failure) => failed_with_evidence(
                "transport.get-behaviour",
                "HTTP transport",
                TRANSPORT_SPEC,
                failure.detail,
                failure.evidence,
            ),
        },
    );
    cases.push(
        match send_gateway(client, GatewayRequest::raw_post(b"{not-json"), mode).await {
            Ok(exchange)
                if exchange.status() == StatusCode::BAD_REQUEST.as_u16()
                    || has_jsonrpc_error_code(&exchange, -32700) =>
            {
                passed(
                    "transport.malformed-json",
                    "HTTP transport",
                    TRANSPORT_SPEC,
                    format!(
                        "malformed JSON returned HTTP {} or a valid Parse Error",
                        exchange.status()
                    ),
                )
            }
            Ok(exchange) => {
                let evidence = evidence_from_exchange(&exchange, protocol_version);
                failed_with_evidence(
                    "transport.malformed-json",
                    "HTTP transport",
                    TRANSPORT_SPEC,
                    format!(
                        "expected HTTP 400 or JSON-RPC Parse Error -32700, got HTTP {}",
                        exchange.status()
                    ),
                    evidence,
                )
            }
            Err(failure) => failed_with_evidence(
                "transport.malformed-json",
                "HTTP transport",
                TRANSPORT_SPEC,
                failure.detail,
                failure.evidence,
            ),
        },
    );
    let malformed_rpc = json!({"id": 22, "method": "ping"}).to_string();
    cases.push(
        match send_gateway(
            client,
            GatewayRequest::raw_post(malformed_rpc.as_bytes()),
            mode,
        )
        .await
        {
            Ok(exchange) if is_invalid_request_response(&exchange) => passed(
                "transport.malformed-jsonrpc",
                "HTTP transport",
                TRANSPORT_SPEC,
                "malformed JSON-RPC returned HTTP 400 or a valid Invalid Request error",
            ),
            Ok(exchange) => {
                let evidence = evidence_from_exchange(&exchange, protocol_version);
                failed_with_evidence(
                    "transport.malformed-jsonrpc",
                    "HTTP transport",
                    TRANSPORT_SPEC,
                    format!(
                        "malformed JSON-RPC returned HTTP {} without HTTP 400 or a valid -32600 Invalid Request envelope",
                        exchange.status()
                    ),
                    evidence,
                )
            }
            Err(failure) => failed_with_evidence(
                "transport.malformed-jsonrpc",
                "HTTP transport",
                TRANSPORT_SPEC,
                failure.detail,
                failure.evidence,
            ),
        },
    );
    cases
}

async fn get_response(
    client: &Client,
    endpoint: &url::Url,
    token: &str,
    protocol_version: &str,
    session_id: Option<&str>,
    mode: StackMode,
) -> std::result::Result<RawExchange, RawExchangeFailure> {
    let mut authorization =
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| RawExchangeFailure {
            detail: "fixture bearer token cannot be encoded as an HTTP header".to_owned(),
            kind: RawExchangeFailureKind::Fixture,
            evidence: unavailable_evidence(
                mode,
                protocol_version,
                "GET request could not be constructed from the fixture bearer token",
            ),
        })?;
    authorization.set_sensitive(true);
    let mut request = client
        .get(endpoint.clone())
        .header(AUTHORIZATION, authorization)
        .header(ACCEPT, "text/event-stream")
        .header(MCP_PROTOCOL_VERSION, protocol_version);
    if let Some(session_id) = session_id {
        let mut session = HeaderValue::from_str(session_id).map_err(|_| RawExchangeFailure {
            detail: "fixture session cannot be encoded as an HTTP header".to_owned(),
            kind: RawExchangeFailureKind::Fixture,
            evidence: unavailable_evidence(
                mode,
                protocol_version,
                "GET request could not be constructed from the fixture session",
            ),
        })?;
        session.set_sensitive(true);
        request = request.header(MCP_SESSION_ID, session);
    }
    let request = request.build().map_err(|_| RawExchangeFailure {
        detail: "GET request could not be constructed".to_owned(),
        kind: RawExchangeFailureKind::Fixture,
        evidence: unavailable_evidence(
            mode,
            protocol_version,
            "GET request builder returned no trustworthy request capture",
        ),
    })?;
    send_raw_headers_only(
        client,
        request,
        mode,
        protocol_version,
        &[token, session_id.unwrap_or("")],
    )
    .await
}

fn is_invalid_request_response(exchange: &Exchange) -> bool {
    exchange.status() == StatusCode::BAD_REQUEST.as_u16()
        || has_jsonrpc_error_code(exchange, -32600)
}

fn has_jsonrpc_error_code(exchange: &Exchange, code: i64) -> bool {
    exchange.message().is_some_and(|message| {
        let Some(object) = message.as_object() else {
            return false;
        };
        let Some(error) = object.get("error").and_then(Value::as_object) else {
            return false;
        };
        object.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
            && object.get("id") == Some(&Value::Null)
            && !object.contains_key("result")
            && error.get("code").and_then(Value::as_i64) == Some(code)
            && error.get("message").and_then(Value::as_str).is_some()
    })
}

fn virtualization_cases(
    tools: &[Value],
    tools_evidence: Option<&GatewayFailureEvidence>,
    config: &GatewayComplianceConfig<'_>,
) -> Vec<GatewayCaseResult> {
    if tools.is_empty() {
        return ["rest", "grpc", "a2a"]
            .into_iter()
            .map(|kind| {
                fixture(
                    format!("virtualization.{kind}-to-mcp"),
                    "Virtualization",
                    TOOLS_SPEC,
                    "tools/list result was unavailable",
                )
            })
            .collect();
    }
    ["REST", "gRPC", "A2A"]
        .into_iter()
        .map(|kind| {
            let matching: Vec<_> = tools
                .iter()
                .filter(|tool| {
                    tool.get("integrationType")
                        .or_else(|| tool.get("integration_type"))
                        .and_then(Value::as_str)
                        .is_some_and(|value| value.eq_ignore_ascii_case(kind))
                })
                .collect();
            if matching.is_empty() {
                return not_applicable(
                    format!("virtualization.{}-to-mcp", kind.to_ascii_lowercase()),
                    "Virtualization",
                    TOOLS_SPEC,
                    format!("live fixture advertises no {kind}-generated MCP tool"),
                );
            }
            if matching
                .iter()
                .all(|tool| tool.get("inputSchema").and_then(Value::as_object).is_some())
            {
                passed(
                    format!("virtualization.{}-to-mcp", kind.to_ascii_lowercase()),
                    "Virtualization",
                    TOOLS_SPEC,
                    format!("all {kind}-generated tools expose object inputSchema values"),
                )
            } else {
                failed_with_evidence(
                    format!("virtualization.{}-to-mcp", kind.to_ascii_lowercase()),
                    "Virtualization",
                    TOOLS_SPEC,
                    format!("a {kind}-generated tool has an invalid inputSchema"),
                    tools_evidence.cloned().unwrap_or_else(|| {
                        unavailable_evidence(
                            config.mode,
                            config.protocol_version,
                            "tools/list validation did not retain its HTTP exchange",
                        )
                    }),
                )
            }
        })
        .collect()
}

fn federation_and_security_gaps(
    mode: StackMode,
    tools: &[Value],
    tools_evidence: Option<&GatewayFailureEvidence>,
    session_assigned: bool,
    protocol_version: &str,
) -> Vec<GatewayCaseResult> {
    let unique_names = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect::<BTreeSet<_>>()
        .len()
        == tools.len();
    vec![
        if tools.is_empty() {
            fixture(
                "federation.exposed-name-uniqueness",
                "Federation",
                TOOLS_SPEC,
                "tools/list result was unavailable",
            )
        } else if unique_names {
            passed(
                "federation.exposed-name-uniqueness",
                "Federation",
                TOOLS_SPEC,
                "federated catalog exposes unique tool names",
            )
        } else {
            failed_with_evidence(
                "federation.exposed-name-uniqueness",
                "Federation",
                TOOLS_SPEC,
                "federated catalog exposes duplicate tool names",
                tools_evidence.cloned().unwrap_or_else(|| {
                    unavailable_evidence(
                        mode,
                        protocol_version,
                        "tools/list validation did not retain its HTTP exchange",
                    )
                }),
            )
        },
        not_applicable(
            "federation.duplicate-upstream-name",
            "Federation",
            TOOLS_SPEC,
            "current live fixture does not register two upstream tools with the same name",
        ),
        not_applicable(
            "preservation.cancellation-progress",
            "Gateway preservation",
            LIFECYCLE_SPEC,
            "current live fixture exposes no cancellable progress-emitting operation",
        ),
        not_applicable(
            "security.tenant-isolation",
            "Security",
            SECURITY_SPEC,
            "current live fixture provisions one tenant and cannot establish cross-tenant isolation",
        ),
        if mode == StackMode::Dataplane {
            not_applicable(
                "security.virtual-server-isolation",
                "Security",
                SECURITY_SPEC,
                "a second live virtual-server fixture is not provisioned",
            )
        } else {
            not_applicable(
                "security.virtual-server-isolation",
                "Security",
                SECURITY_SPEC,
                "control-plane mode uses the raw /mcp route",
            )
        },
        if session_assigned {
            not_applicable(
                "session.expired-session",
                "Session handling",
                TRANSPORT_SPEC,
                "fixture does not expose a deterministic short session TTL",
            )
        } else {
            not_applicable(
                "session.expired-session",
                "Session handling",
                TRANSPORT_SPEC,
                "server selected stateless operation and does not expire session IDs",
            )
        },
    ]
}

async fn delete_and_reuse_session(
    client: &mut GatewayClient,
    mode: StackMode,
) -> Vec<GatewayCaseResult> {
    if client.session_id().is_none() {
        return vec![
            not_applicable(
                "session.delete",
                "Session handling",
                TRANSPORT_SPEC,
                "server did not establish a stateful session",
            ),
            not_applicable(
                "session.deleted-session",
                "Session handling",
                TRANSPORT_SPEC,
                "server did not establish a stateful session",
            ),
        ];
    }
    let deleted = send_gateway(client, GatewayRequest::delete(), mode).await;
    let mut cases = Vec::new();
    match deleted {
        Ok(exchange) if matches!(exchange.status(), 200 | 202 | 204) => cases.push(passed(
            "session.delete",
            "Session handling",
            TRANSPORT_SPEC,
            format!("DELETE returned HTTP {}", exchange.status()),
        )),
        Ok(exchange) if exchange.status() == 405 => {
            cases.push(not_applicable(
                "session.delete",
                "Session handling",
                TRANSPORT_SPEC,
                "server does not permit client-initiated session termination (HTTP 405)",
            ));
            cases.push(not_applicable(
                "session.deleted-session",
                "Session handling",
                TRANSPORT_SPEC,
                "session was not deleted because the server returned HTTP 405",
            ));
            return cases;
        }
        Ok(exchange) => {
            let evidence = evidence_from_exchange(&exchange, client.protocol_version());
            cases.push(failed_with_evidence(
                "session.delete",
                "Session handling",
                TRANSPORT_SPEC,
                format!("DELETE returned unexpected HTTP {}", exchange.status()),
                evidence,
            ));
            return cases;
        }
        Err(failure) => {
            cases.push(failed_with_evidence(
                "session.delete",
                "Session handling",
                TRANSPORT_SPEC,
                failure.detail,
                failure.evidence,
            ));
            return cases;
        }
    }
    cases.push(
        match send_gateway(
            client,
            GatewayRequest::request("ping", None, json!(30)).unchecked(),
            mode,
        )
        .await
        {
            Ok(exchange) if exchange.status() == 404 => passed(
                "session.deleted-session",
                "Session handling",
                TRANSPORT_SPEC,
                "deleted session was rejected with HTTP 404",
            ),
            Ok(exchange) => {
                let evidence = evidence_from_exchange(&exchange, client.protocol_version());
                failed_with_evidence(
                    "session.deleted-session",
                    "Session handling",
                    TRANSPORT_SPEC,
                    format!("deleted session returned HTTP {}", exchange.status()),
                    evidence,
                )
            }
            Err(failure) => failed_with_evidence(
                "session.deleted-session",
                "Session handling",
                TRANSPORT_SPEC,
                failure.detail,
                failure.evidence,
            ),
        },
    );
    cases
}

fn add_blocked_cases(cases: &mut Vec<GatewayCaseResult>, reason: &str) {
    for (name, category, spec) in [
        (
            "protocol.initialize-result",
            "Protocol negotiation",
            LIFECYCLE_SPEC,
        ),
        (
            "protocol.version-negotiation",
            "Protocol negotiation",
            LIFECYCLE_SPEC,
        ),
        (
            "protocol.capability-negotiation",
            "Protocol negotiation",
            LIFECYCLE_SPEC,
        ),
        (
            "protocol.server-info",
            "Protocol negotiation",
            LIFECYCLE_SPEC,
        ),
        ("session.creation", "Session handling", TRANSPORT_SPEC),
        (
            "protocol.initialized-notification",
            "Protocol negotiation",
            LIFECYCLE_SPEC,
        ),
        ("protocol.ping", "Utilities", PING_SPEC),
        ("session.reuse", "Session handling", TRANSPORT_SPEC),
        (
            "preservation.tools-list",
            "Gateway preservation",
            TOOLS_SPEC,
        ),
        (
            "preservation.tool-schema-stability",
            "Gateway preservation",
            TOOLS_SPEC,
        ),
        (
            "preservation.tool-result",
            "Gateway preservation",
            TOOLS_SPEC,
        ),
        (
            "federation.resources-aggregation",
            "Federation",
            RESOURCES_SPEC,
        ),
        ("federation.prompts-aggregation", "Federation", PROMPTS_SPEC),
        (
            "transport.invalid-protocol-version",
            "HTTP transport",
            TRANSPORT_SPEC,
        ),
        (
            "session.invalid-session",
            "Session handling",
            TRANSPORT_SPEC,
        ),
        ("transport.get-behaviour", "HTTP transport", TRANSPORT_SPEC),
        ("transport.malformed-json", "HTTP transport", TRANSPORT_SPEC),
        (
            "transport.malformed-jsonrpc",
            "HTTP transport",
            TRANSPORT_SPEC,
        ),
        ("virtualization.rest-to-mcp", "Virtualization", TOOLS_SPEC),
        ("virtualization.grpc-to-mcp", "Virtualization", TOOLS_SPEC),
        ("virtualization.a2a-to-mcp", "Virtualization", TOOLS_SPEC),
        (
            "federation.exposed-name-uniqueness",
            "Federation",
            TOOLS_SPEC,
        ),
        (
            "federation.duplicate-upstream-name",
            "Federation",
            TOOLS_SPEC,
        ),
        (
            "preservation.cancellation-progress",
            "Gateway preservation",
            LIFECYCLE_SPEC,
        ),
        ("security.tenant-isolation", "Security", SECURITY_SPEC),
        (
            "security.virtual-server-isolation",
            "Security",
            SECURITY_SPEC,
        ),
        (
            "session.expired-session",
            "Session handling",
            TRANSPORT_SPEC,
        ),
        ("session.delete", "Session handling", TRANSPORT_SPEC),
        (
            "session.deleted-session",
            "Session handling",
            TRANSPORT_SPEC,
        ),
    ] {
        cases.push(fixture(name, category, spec, reason));
    }
}

/// Deterministically renders one mode's gateway report.
#[must_use]
pub fn render_gateway_report(report: &GatewayComplianceReport) -> String {
    let mut output = format!(
        "# MCP Gateway Compliance: {}\n\n- Specification: `{}`\n\n",
        markdown(&report.mode),
        markdown(&report.specification_version)
    );
    let mut counts = BTreeMap::new();
    for case in &report.cases {
        *counts.entry(case.status.label()).or_insert(0_usize) += 1;
    }
    output.push_str("| Status | Cases |\n|---|---:|\n");
    for status in [
        GatewayCaseStatus::Passed,
        GatewayCaseStatus::Failed,
        GatewayCaseStatus::NotApplicable,
        GatewayCaseStatus::FixtureFailure,
    ] {
        output.push_str(&format!(
            "| {} | {} |\n",
            status.label(),
            counts.get(status.label()).copied().unwrap_or_default()
        ));
    }
    output.push_str(
        "\n| Case | Category | Status | Specification | Detail |\n|---|---|---|---|---|\n",
    );
    let mut cases = report.cases.iter().collect::<Vec<_>>();
    cases.sort_by(|left, right| left.name.cmp(&right.name));
    for case in cases {
        output.push_str(&format!(
            "| {} | {} | {} | [{}]({}) | {} |\n",
            markdown(&case.name),
            markdown(&case.category),
            case.status.label(),
            markdown(&format!("MCP {}", report.specification_version)),
            case.specification,
            markdown(&case.detail)
        ));
    }
    let failures = report
        .cases
        .iter()
        .filter(|case| case.status == GatewayCaseStatus::Failed)
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        output.push_str("\n## Failure evidence\n\n");
        for case in failures {
            output.push_str(&format!("### {}\n\n", markdown(&case.name)));
            match &case.failure_evidence {
                Some(evidence) => render_failure_evidence(&mut output, evidence),
                None => output.push_str(
                    "| Field | Value |\n|---|---|\n| Stack mode | unavailable: no diagnostic was recorded |\n| Protocol version | unavailable: no diagnostic was recorded |\n| Request availability | unavailable |\n| Request method | unavailable: no diagnostic was recorded |\n| Request URL | unavailable: no diagnostic was recorded |\n| Request headers | unavailable: no diagnostic was recorded |\n| Request body | unavailable: no diagnostic was recorded |\n| Response availability | unavailable |\n| Response status | unavailable: no diagnostic was recorded |\n| Response headers | unavailable: no diagnostic was recorded |\n| Response body | unavailable: no diagnostic was recorded |\n\n",
                ),
            }
        }
    }
    while output.ends_with("\n\n") {
        output.pop();
    }
    output
}

fn render_failure_evidence(output: &mut String, evidence: &GatewayFailureEvidence) {
    output.push_str("| Field | Value |\n|---|---|\n");
    output.push_str(&format!(
        "| Stack mode | {} |\n| Protocol version | {} |\n",
        markdown(&evidence.stack_mode),
        markdown(&evidence.protocol_version)
    ));
    match &evidence.request {
        GatewayRequestEvidence::Captured {
            method,
            url,
            headers,
            body,
        } => {
            output.push_str(&format!(
                "| Request availability | captured |\n| Request method | {} |\n| Request URL | {} |\n| Request headers | {} |\n| Request body | {} |\n",
                markdown(method),
                markdown(url),
                markdown(&serde_json::to_string(headers).unwrap_or_else(|_| "unavailable: serialization failed".to_owned())),
                markdown(body.as_deref().unwrap_or("<no body>")),
            ));
        }
        GatewayRequestEvidence::Unavailable { reason } => {
            let reason = markdown(reason);
            output.push_str(&format!(
                "| Request availability | unavailable |\n| Request method | unavailable: {reason} |\n| Request URL | unavailable: {reason} |\n| Request headers | unavailable: {reason} |\n| Request body | unavailable: {reason} |\n"
            ));
        }
    }
    match &evidence.response {
        GatewayResponseEvidence::Captured {
            status,
            headers,
            body,
        } => {
            output.push_str(&format!(
                "| Response availability | captured |\n| Response status | {status} |\n| Response headers | {} |\n| Response body | {} |\n",
                markdown(&serde_json::to_string(headers).unwrap_or_else(|_| "unavailable: serialization failed".to_owned())),
                markdown(body),
            ));
        }
        GatewayResponseEvidence::HeadersCaptured {
            status,
            headers,
            reason,
        } => {
            output.push_str(&format!(
                "| Response availability | headers captured |
| Response status | {status} |
| Response headers | {} |
| Response body | unavailable: {} |
",
                markdown(
                    &serde_json::to_string(headers)
                        .unwrap_or_else(|_| "unavailable: serialization failed".to_owned())
                ),
                markdown(reason),
            ));
        }
        GatewayResponseEvidence::Unavailable { reason } => {
            let reason = markdown(reason);
            output.push_str(&format!(
                "| Response availability | unavailable |\n| Response status | unavailable: {reason} |\n| Response headers | unavailable: {reason} |\n| Response body | unavailable: {reason} |\n"
            ));
        }
    }
    output.push('\n');
}

/// Writes Markdown and JSON gateway result artifacts.
pub fn write_gateway_reports(
    markdown_path: &Path,
    json_path: &Path,
    report: &GatewayComplianceReport,
) -> Result<()> {
    for path in [markdown_path, json_path] {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create gateway report directory {parent:?}"))?;
        }
    }
    fs::write(markdown_path, render_gateway_report(report))
        .with_context(|| format!("failed to write gateway report {markdown_path:?}"))?;
    fs::write(
        json_path,
        serde_json::to_vec_pretty(report).context("failed to serialize gateway report")?,
    )
    .with_context(|| format!("failed to write gateway report {json_path:?}"))?;
    Ok(())
}

fn passed(
    name: impl Into<String>,
    category: impl Into<String>,
    specification: impl Into<String>,
    detail: impl Into<String>,
) -> GatewayCaseResult {
    case(
        name,
        category,
        GatewayCaseStatus::Passed,
        specification,
        detail,
    )
}

fn failed(
    name: impl Into<String>,
    category: impl Into<String>,
    specification: impl Into<String>,
    detail: impl Into<String>,
) -> GatewayCaseResult {
    case(
        name,
        category,
        GatewayCaseStatus::Failed,
        specification,
        detail,
    )
}

fn failed_with_evidence(
    name: impl Into<String>,
    category: impl Into<String>,
    specification: impl Into<String>,
    detail: impl Into<String>,
    evidence: GatewayFailureEvidence,
) -> GatewayCaseResult {
    let mut result = failed(name, category, specification, detail);
    result.failure_evidence = Some(evidence);
    result
}

fn not_applicable(
    name: impl Into<String>,
    category: impl Into<String>,
    specification: impl Into<String>,
    detail: impl Into<String>,
) -> GatewayCaseResult {
    case(
        name,
        category,
        GatewayCaseStatus::NotApplicable,
        specification,
        detail,
    )
}

fn fixture(
    name: impl Into<String>,
    category: impl Into<String>,
    specification: impl Into<String>,
    detail: impl Into<String>,
) -> GatewayCaseResult {
    case(
        name,
        category,
        GatewayCaseStatus::FixtureFailure,
        specification,
        detail,
    )
}

fn case(
    name: impl Into<String>,
    category: impl Into<String>,
    status: GatewayCaseStatus,
    specification: impl Into<String>,
    detail: impl Into<String>,
) -> GatewayCaseResult {
    GatewayCaseResult {
        name: name.into(),
        category: category.into(),
        status,
        specification: specification.into(),
        detail: sanitize(&detail.into()),
        failure_evidence: None,
    }
}

fn mode_label(mode: StackMode) -> &'static str {
    match mode {
        StackMode::Controlplane => "controlplane",
        StackMode::Dataplane => "dataplane",
    }
}

fn markdown(value: &str) -> String {
    sanitize(value)
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn sanitize(value: &str) -> String {
    value
        .replace('\r', "\\r")
        .replace('\n', "\\n")
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}
