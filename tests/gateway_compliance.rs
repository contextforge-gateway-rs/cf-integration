use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Request, Response, StatusCode};
use axum::routing::any;
use cf_integration::cli::StackMode;
use cf_integration::gateway::{MCP_PROTOCOL_VERSION, MCP_SESSION_ID};
use cf_integration::gateway_compliance::{
    GATEWAY_SPEC_VERSION, GatewayCaseResult, GatewayCaseStatus, GatewayComplianceConfig,
    GatewayComplianceReport, GatewayFailureEvidence, GatewayRequestEvidence,
    GatewayResponseEvidence, render_gateway_report, run_gateway_compliance, write_gateway_reports,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const TOKEN: &str = "valid-token";
const WRONG_TOKEN: &str = "wrong-token";
const SESSION: &str = "gateway-session";
const SPEC_VERSION: &str = GATEWAY_SPEC_VERSION;

#[derive(Clone, Copy)]
enum RawBackendMarker {
    Dataplane,
    Fallback,
    Invalid(&'static str),
    Multiple,
}

#[derive(Clone)]
struct MockBehavior {
    advertise_tools: bool,
    delete_status: StatusCode,
    duplicate_tools: bool,
    fail_initialize: bool,
    get_content_type: Option<&'static str>,
    issue_session: bool,
    malformed_error_envelope: bool,
    malformed_initialize_metadata: bool,
    malformed_request_status: StatusCode,
    noncompliant_security: bool,
    open_get_stream: bool,
    raw_backend_marker: RawBackendMarker,
}

impl Default for MockBehavior {
    fn default() -> Self {
        Self {
            advertise_tools: true,
            delete_status: StatusCode::NO_CONTENT,
            duplicate_tools: false,
            fail_initialize: false,
            get_content_type: None,
            issue_session: true,
            malformed_error_envelope: false,
            malformed_initialize_metadata: false,
            malformed_request_status: StatusCode::BAD_REQUEST,
            noncompliant_security: false,
            open_get_stream: false,
            raw_backend_marker: RawBackendMarker::Dataplane,
        }
    }
}

#[derive(Clone)]
struct MockState {
    behavior: MockBehavior,
    deleted: Arc<AtomicBool>,
    paths: Arc<Mutex<Vec<String>>>,
}

struct MockGateway {
    base_url: String,
    paths: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl MockGateway {
    async fn start(fail_initialize: bool) -> Self {
        Self::start_with_behavior(MockBehavior {
            fail_initialize,
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_with_delete_status(fail_initialize: bool, delete_status: StatusCode) -> Self {
        Self::start_with_behavior(MockBehavior {
            fail_initialize,
            delete_status,
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_stateless() -> Self {
        Self::start_with_behavior(MockBehavior {
            issue_session: false,
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_with_get_content_type(content_type: &'static str) -> Self {
        Self::start_with_behavior(MockBehavior {
            get_content_type: Some(content_type),
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_with_open_get_stream() -> Self {
        Self::start_with_behavior(MockBehavior {
            get_content_type: Some("text/event-stream"),
            open_get_stream: true,
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_without_tools_capability() -> Self {
        Self::start_with_behavior(MockBehavior {
            advertise_tools: false,
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_with_security_failures() -> Self {
        Self::start_with_behavior(MockBehavior {
            noncompliant_security: true,
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_with_malformed_initialize_metadata() -> Self {
        Self::start_with_behavior(MockBehavior {
            malformed_initialize_metadata: true,
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_with_duplicate_tools() -> Self {
        Self::start_with_behavior(MockBehavior {
            duplicate_tools: true,
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_with_malformed_request_status(status: StatusCode) -> Self {
        Self::start_with_behavior(MockBehavior {
            malformed_request_status: status,
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_with_malformed_error_envelope() -> Self {
        Self::start_with_behavior(MockBehavior {
            malformed_error_envelope: true,
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_with_raw_backend_marker(marker: RawBackendMarker) -> Self {
        Self::start_with_behavior(MockBehavior {
            get_content_type: Some("text/event-stream"),
            raw_backend_marker: marker,
            ..MockBehavior::default()
        })
        .await
    }

    async fn start_with_behavior(behavior: MockBehavior) -> Self {
        let paths = Arc::new(Mutex::new(Vec::new()));
        let state = MockState {
            behavior,
            deleted: Arc::new(AtomicBool::new(false)),
            paths: Arc::clone(&paths),
        };
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock gateway listener should bind");
        let address = listener
            .local_addr()
            .expect("mock gateway should have a local address");
        let app = Router::new().fallback(any(mock_gateway)).with_state(state);
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock gateway should run");
        });
        Self {
            base_url: format!("http://{address}"),
            paths,
            task,
        }
    }

    fn paths(&self) -> Vec<String> {
        self.paths
            .lock()
            .expect("mock path observations should not be poisoned")
            .clone()
    }
}

impl Drop for MockGateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn mock_gateway(State(state): State<MockState>, request: Request<Body>) -> Response<Body> {
    let (parts, body) = request.into_parts();
    state
        .paths
        .lock()
        .expect("mock path observations should not be poisoned")
        .push(parts.uri.path().to_owned());
    let body = to_bytes(body, 1024 * 1024)
        .await
        .expect("mock gateway request should fit the test limit");

    let authorization = header(&parts.headers, "authorization");
    if authorization.is_none() {
        return marked_empty_response(
            if state.behavior.noncompliant_security {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::UNAUTHORIZED
            },
            state.behavior.raw_backend_marker,
        );
    }
    if !state.behavior.noncompliant_security
        && header(&parts.headers, "origin") == Some("https://attacker.invalid")
    {
        return marked_empty_response(StatusCode::FORBIDDEN, state.behavior.raw_backend_marker);
    }
    if !state.behavior.noncompliant_security && authorization != Some(&format!("Bearer {TOKEN}")) {
        return empty_response(StatusCode::FORBIDDEN);
    }

    if state.behavior.issue_session
        && parts.method.as_str() != "POST"
        && header(&parts.headers, MCP_SESSION_ID) != Some(SESSION)
    {
        return empty_response(StatusCode::BAD_REQUEST);
    }
    match parts.method.as_str() {
        "GET" => {
            return state.behavior.get_content_type.map_or_else(
                || empty_response(StatusCode::METHOD_NOT_ALLOWED),
                |content_type| {
                    response_with_content_type(
                        StatusCode::OK,
                        content_type,
                        state.behavior.open_get_stream,
                        state.behavior.raw_backend_marker,
                    )
                },
            );
        }
        "DELETE" => {
            if state.behavior.delete_status != StatusCode::METHOD_NOT_ALLOWED {
                state.deleted.store(true, Ordering::SeqCst);
            }
            return empty_response(state.behavior.delete_status);
        }
        "POST" => {}
        _ => return empty_response(StatusCode::METHOD_NOT_ALLOWED),
    }

    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return empty_response(state.behavior.malformed_request_status);
    };
    if payload.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        if state.behavior.malformed_request_status.is_server_error() {
            return empty_response(state.behavior.malformed_request_status);
        }
        if state.behavior.malformed_error_envelope {
            return json_response(
                StatusCode::OK,
                json!({
                    "jsonrpc": "2.0",
                    "id": 22,
                    "error": {"code": -32600, "message": "Invalid Request"}
                }),
                None,
            );
        }
        return json_response(
            StatusCode::OK,
            json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32600, "message": "Invalid Request"}
            }),
            None,
        );
    }

    let method = payload.get("method").and_then(Value::as_str).unwrap_or("");
    if method == "initialize" {
        if state.behavior.fail_initialize {
            return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
        let session = state.behavior.issue_session.then_some(SESSION);
        let capabilities = if state.behavior.malformed_initialize_metadata {
            json!({"tools": null})
        } else if state.behavior.advertise_tools {
            json!({"tools": {}})
        } else {
            json!({})
        };
        let server_info = if state.behavior.malformed_initialize_metadata {
            json!({})
        } else {
            json!({"name": "mock-gateway", "version": "1.0.0"})
        };
        return json_response(
            StatusCode::OK,
            rpc_result(
                &payload,
                json!({
                    "protocolVersion": SPEC_VERSION,
                    "capabilities": capabilities,
                    "serverInfo": server_info
                }),
            ),
            session,
        );
    }
    if state.behavior.issue_session {
        match header(&parts.headers, MCP_SESSION_ID) {
            Some("invalid-session") => return empty_response(StatusCode::NOT_FOUND),
            Some(SESSION) => {}
            _ => return empty_response(StatusCode::BAD_REQUEST),
        }
    }
    if method == "notifications/initialized" {
        return empty_response(StatusCode::ACCEPTED);
    }
    if header(&parts.headers, MCP_PROTOCOL_VERSION) == Some("unsupported-version") {
        return empty_response(StatusCode::BAD_REQUEST);
    }
    if !state.behavior.noncompliant_security && state.deleted.load(Ordering::SeqCst) {
        return empty_response(StatusCode::NOT_FOUND);
    }

    match method {
        "tools/list" => {
            let mut tools = vec![json!({
                "name": "echo",
                "description": "safe test echo",
                "inputSchema": {"type": "object"}
            })];
            if state.behavior.duplicate_tools {
                tools.push(tools[0].clone());
            }
            json_response(
                StatusCode::OK,
                rpc_result(&payload, json!({"tools": tools})),
                None,
            )
        }
        "tools/call" => json_response(
            StatusCode::OK,
            rpc_result(
                &payload,
                json!({"content": [{"type": "text", "text": "cf-integration"}]}),
            ),
            None,
        ),
        _ => json_response(StatusCode::OK, rpc_result(&payload, json!({})), None),
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn rpc_result(request: &Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request.get("id").cloned().unwrap_or(Value::Null),
        "result": result
    })
}

fn empty_response(status: StatusCode) -> Response<Body> {
    marked_empty_response(status, RawBackendMarker::Dataplane)
}

fn marked_empty_response(status: StatusCode, marker: RawBackendMarker) -> Response<Body> {
    marked_response(status, Body::empty(), marker)
}

fn response_with_content_type(
    status: StatusCode,
    content_type: &str,
    open_stream: bool,
    marker: RawBackendMarker,
) -> Response<Body> {
    let body = if open_stream {
        Body::from_stream(tokio_stream::pending::<Result<Bytes, Infallible>>())
    } else {
        Body::empty()
    };
    let mut response = marked_response(status, body, marker);
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_str(content_type).expect("mock content type should be valid"),
    );
    response
}

fn marked_response(status: StatusCode, body: Body, marker: RawBackendMarker) -> Response<Body> {
    let mut response = Response::builder()
        .status(status)
        .body(body)
        .expect("mock response should build");
    let headers = response.headers_mut();
    let values: &[&str] = match marker {
        RawBackendMarker::Dataplane => &["dataplane"],
        RawBackendMarker::Fallback => &["controlplane-fallback"],
        RawBackendMarker::Invalid(value) => &[value],
        RawBackendMarker::Multiple => &["dataplane", "controlplane-fallback"],
    };
    for value in values {
        headers.append("x-cf-integration-backend", HeaderValue::from_static(value));
    }
    response
}

fn json_response(status: StatusCode, body: Value, session: Option<&str>) -> Response<Body> {
    let mut builder = Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("x-cf-integration-backend", "dataplane");
    if let Some(session) = session {
        builder = builder.header(MCP_SESSION_ID, session);
    }
    builder
        .body(Body::from(body.to_string()))
        .expect("mock JSON response should build")
}

fn config<'a>(
    gateway: &'a MockGateway,
    wrong_scope_token: Option<&'a str>,
) -> GatewayComplianceConfig<'a> {
    GatewayComplianceConfig {
        mode: StackMode::Dataplane,
        base_url: &gateway.base_url,
        server_id: "virtual/server",
        bearer_token: TOKEN,
        wrong_scope_token,
        protocol_version: SPEC_VERSION,
    }
}

fn case<'a>(report: &'a GatewayComplianceReport, name: &str) -> &'a GatewayCaseResult {
    report
        .cases
        .iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("report should contain {name}"))
}

#[tokio::test]
async fn dataplane_suite_exercises_the_complete_gateway_lifecycle() {
    let gateway = MockGateway::start(false).await;

    let report = run_gateway_compliance(&config(&gateway, Some(WRONG_TOKEN)))
        .await
        .expect("mock gateway suite should run");

    assert!(report.is_compliant(), "{:#?}", report.cases);
    assert_eq!(report.mode, "dataplane");
    assert_eq!(
        case(&report, "security.authorization-wrong-server").status,
        GatewayCaseStatus::Passed
    );
    assert_eq!(
        case(&report, "preservation.tool-result").status,
        GatewayCaseStatus::Passed
    );
    assert_eq!(
        case(&report, "protocol.ping").status,
        GatewayCaseStatus::Passed
    );
    assert_eq!(
        case(&report, "session.reuse").status,
        GatewayCaseStatus::Passed
    );
    assert_eq!(
        case(&report, "session.deleted-session").status,
        GatewayCaseStatus::Passed
    );
    let unique_names = report
        .cases
        .iter()
        .map(|case| case.name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        unique_names.len(),
        report.cases.len(),
        "case names must be unique"
    );
    assert_eq!(report.cases.len(), 33, "case catalog size must stay stable");
    assert!(
        gateway
            .paths()
            .iter()
            .all(|path| path == "/servers/virtual%2Fserver/mcp")
    );
}

#[tokio::test]
async fn controlplane_suite_uses_raw_mcp_and_skips_path_scoped_authorization() {
    let gateway = MockGateway::start(false).await;
    let config = GatewayComplianceConfig {
        mode: StackMode::Controlplane,
        base_url: &gateway.base_url,
        server_id: "unused-server",
        bearer_token: TOKEN,
        wrong_scope_token: None,
        protocol_version: SPEC_VERSION,
    };

    let report = run_gateway_compliance(&config)
        .await
        .expect("control-plane mock gateway suite should run");

    assert!(report.is_compliant(), "{:#?}", report.cases);
    assert_eq!(report.mode, "controlplane");
    assert_eq!(
        case(&report, "security.authorization-wrong-server").status,
        GatewayCaseStatus::NotApplicable
    );
    assert_eq!(report.cases.len(), 33);
    assert!(gateway.paths().iter().all(|path| path == "/mcp"));
}

#[tokio::test]
async fn live_product_failures_include_complete_redacted_exchange_evidence() {
    let gateway = MockGateway::start_with_security_failures().await;

    let report = run_gateway_compliance(&config(&gateway, Some(WRONG_TOKEN)))
        .await
        .expect("noncompliant mock suite should finish");

    for (name, expected_status) in [
        ("security.authentication-required", 403),
        ("security.invalid-origin", 200),
        ("security.authorization-wrong-server", 200),
        ("session.deleted-session", 200),
    ] {
        let result = case(&report, name);
        assert_eq!(result.status, GatewayCaseStatus::Failed, "{name}");
        let evidence = result
            .failure_evidence
            .as_ref()
            .unwrap_or_else(|| panic!("{name} should include failure evidence"));
        assert_eq!(evidence.stack_mode, "dataplane", "{name}");
        assert_eq!(evidence.protocol_version, SPEC_VERSION, "{name}");
        match &evidence.request {
            GatewayRequestEvidence::Captured {
                method,
                url,
                headers,
                body,
            } => {
                assert_eq!(method, "POST", "{name}");
                assert!(
                    url.ends_with("/servers/virtual%2Fserver/mcp"),
                    "{name}: {url}"
                );
                assert!(
                    body.as_deref().is_some_and(|body| !body.is_empty()),
                    "{name}"
                );
                assert!(
                    headers
                        .get("authorization")
                        .is_none_or(|value| value == "<redacted>"),
                    "{name}: {headers:?}"
                );
                assert!(
                    headers.values().all(|value| !value.contains(TOKEN)),
                    "{name}: {headers:?}"
                );
                assert!(
                    headers.values().all(|value| !value.contains(SESSION)),
                    "{name}: {headers:?}"
                );
            }
            GatewayRequestEvidence::Unavailable { reason } => {
                panic!("{name} request should be captured, got unavailable: {reason}")
            }
        }
        match &evidence.response {
            GatewayResponseEvidence::Captured {
                status,
                headers,
                body,
            } => {
                assert_eq!(*status, expected_status, "{name}");
                assert!(
                    headers.values().all(|value| !value.contains(TOKEN)),
                    "{name}"
                );
                assert!(
                    headers.values().all(|value| !value.contains(SESSION)),
                    "{name}"
                );
                assert!(!body.contains(TOKEN), "{name}");
                assert!(!body.contains(SESSION), "{name}");
            }
            GatewayResponseEvidence::Unavailable { reason } => {
                panic!("{name} response should be captured, got unavailable: {reason}")
            }
            GatewayResponseEvidence::HeadersCaptured { reason, .. } => {
                panic!("{name} response body should be captured, got headers only: {reason}")
            }
        }
    }
    assert!(
        report
            .cases
            .iter()
            .filter(|case| case.status == GatewayCaseStatus::Failed)
            .all(|case| case.failure_evidence.is_some())
    );
}

#[tokio::test]
async fn unavailable_wrong_scope_fixture_is_not_reported_as_a_product_failure() {
    let gateway = MockGateway::start(false).await;

    let report = run_gateway_compliance(&config(&gateway, None))
        .await
        .expect("suite should finish with a fixture result");

    let result = case(&report, "security.authorization-wrong-server");
    assert_eq!(result.status, GatewayCaseStatus::FixtureFailure);
    assert!(result.detail.contains("fixture is unavailable"));
    assert!(!report.is_compliant());
}

#[tokio::test]
async fn server_may_disallow_client_initiated_session_termination() {
    let gateway =
        MockGateway::start_with_delete_status(false, StatusCode::METHOD_NOT_ALLOWED).await;

    let report = run_gateway_compliance(&config(&gateway, Some(WRONG_TOKEN)))
        .await
        .expect("mock gateway suite should run");

    assert!(report.is_compliant(), "{:#?}", report.cases);
    assert_eq!(
        case(&report, "session.delete").status,
        GatewayCaseStatus::NotApplicable
    );
    assert_eq!(
        case(&report, "session.deleted-session").status,
        GatewayCaseStatus::NotApplicable
    );
}

#[tokio::test]
async fn stateless_server_marks_session_lifecycle_cases_not_applicable() {
    let gateway = MockGateway::start_stateless().await;

    let report = run_gateway_compliance(&config(&gateway, Some(WRONG_TOKEN)))
        .await
        .expect("stateless mock gateway suite should run");

    assert!(report.is_compliant(), "{:#?}", report.cases);
    for name in [
        "session.creation",
        "session.reuse",
        "session.invalid-session",
        "session.expired-session",
        "session.delete",
        "session.deleted-session",
    ] {
        assert_eq!(
            case(&report, name).status,
            GatewayCaseStatus::NotApplicable,
            "{name} should be not applicable for a stateless server"
        );
    }
}

#[tokio::test]
async fn get_200_requires_an_sse_content_type() {
    let json_gateway = MockGateway::start_with_get_content_type("application/json").await;
    let json_report = run_gateway_compliance(&config(&json_gateway, Some(WRONG_TOKEN)))
        .await
        .expect("JSON GET mock suite should run");
    assert_eq!(
        case(&json_report, "transport.get-behaviour").status,
        GatewayCaseStatus::Failed
    );
    let evidence = case(&json_report, "transport.get-behaviour")
        .failure_evidence
        .as_ref()
        .expect("GET validation failure should retain the raw HTTP exchange");
    assert!(matches!(
        evidence.request,
        GatewayRequestEvidence::Captured { .. }
    ));
    assert!(matches!(
        evidence.response,
        GatewayResponseEvidence::HeadersCaptured { status: 200, .. }
    ));
    let evidence_json = serde_json::to_value(evidence).expect("GET evidence should serialize");
    assert_eq!(
        evidence_json.pointer("/response/availability"),
        Some(&Value::String("headers-captured".to_owned()))
    );
    assert_eq!(evidence_json.pointer("/response/body"), Some(&Value::Null));

    let sse_gateway =
        MockGateway::start_with_get_content_type("text/event-stream; charset=utf-8").await;
    let sse_report = run_gateway_compliance(&config(&sse_gateway, Some(WRONG_TOKEN)))
        .await
        .expect("SSE GET mock suite should run");
    assert_eq!(
        case(&sse_report, "transport.get-behaviour").status,
        GatewayCaseStatus::Passed
    );
}

#[tokio::test]
async fn gateway_compliance_does_not_drain_an_open_sse_get() {
    let gateway = MockGateway::start_with_open_get_stream().await;

    let report = tokio::time::timeout(
        Duration::from_secs(2),
        run_gateway_compliance(&config(&gateway, Some(WRONG_TOKEN))),
    )
    .await
    .expect("gateway suite must not wait for an SSE stream to close")
    .expect("open-SSE mock suite should run");

    assert_eq!(
        case(&report, "transport.get-behaviour").status,
        GatewayCaseStatus::Passed
    );
}

#[tokio::test]
async fn initialize_metadata_requires_well_formed_tools_and_server_info() {
    let gateway = MockGateway::start_with_malformed_initialize_metadata().await;

    let report = run_gateway_compliance(&config(&gateway, Some(WRONG_TOKEN)))
        .await
        .expect("malformed initialize metadata should be reported");

    for name in ["protocol.capability-negotiation", "protocol.server-info"] {
        assert_eq!(
            case(&report, name).status,
            GatewayCaseStatus::Failed,
            "{name}"
        );
    }
}

#[tokio::test]
async fn duplicate_tool_names_fail_the_federation_case_instead_of_becoming_fixture_failure() {
    let gateway = MockGateway::start_with_duplicate_tools().await;

    let report = run_gateway_compliance(&config(&gateway, Some(WRONG_TOKEN)))
        .await
        .expect("duplicate tools should be represented in the report");

    assert_eq!(
        case(&report, "preservation.tools-list").status,
        GatewayCaseStatus::Failed
    );
    assert_eq!(
        case(&report, "federation.exposed-name-uniqueness").status,
        GatewayCaseStatus::Failed
    );
}

#[tokio::test]
async fn malformed_inputs_do_not_accept_server_errors_or_invalid_error_envelopes() {
    let crashing_gateway =
        MockGateway::start_with_malformed_request_status(StatusCode::INTERNAL_SERVER_ERROR).await;
    let crash_report = run_gateway_compliance(&config(&crashing_gateway, Some(WRONG_TOKEN)))
        .await
        .expect("server errors should be represented in the report");
    for name in ["transport.malformed-json", "transport.malformed-jsonrpc"] {
        assert_eq!(
            case(&crash_report, name).status,
            GatewayCaseStatus::Failed,
            "{name} must not accept HTTP 500"
        );
    }

    let malformed_gateway = MockGateway::start_with_malformed_error_envelope().await;
    let malformed_report = run_gateway_compliance(&config(&malformed_gateway, Some(WRONG_TOKEN)))
        .await
        .expect("malformed error envelope should be represented in the report");
    assert_eq!(
        case(&malformed_report, "transport.malformed-jsonrpc").status,
        GatewayCaseStatus::Failed
    );
    assert_eq!(
        case(&malformed_report, "transport.malformed-jsonrpc").detail,
        "malformed JSON-RPC returned HTTP 200 without HTTP 400 or a valid -32600 Invalid Request envelope"
    );
}

#[tokio::test]
async fn raw_dataplane_cases_reject_fallback_forged_and_duplicate_backend_markers() {
    const FORGED: &str = "forged-marker-secret";
    for marker in [
        RawBackendMarker::Fallback,
        RawBackendMarker::Invalid(FORGED),
        RawBackendMarker::Multiple,
    ] {
        let gateway = MockGateway::start_with_raw_backend_marker(marker).await;
        let report = run_gateway_compliance(&config(&gateway, Some(WRONG_TOKEN)))
            .await
            .expect("backend identity failures should be represented in the report");
        for name in [
            "security.authentication-required",
            "security.invalid-origin",
            "transport.get-behaviour",
        ] {
            let result = case(&report, name);
            assert_eq!(result.status, GatewayCaseStatus::Failed, "{name}");
            assert!(
                result.detail.contains("dataplane response backend marker"),
                "{}",
                result.detail
            );
        }
        let serialized = serde_json::to_string(&report).expect("report should serialize");
        assert!(!serialized.contains(FORGED));
    }
}

#[tokio::test]
async fn initialize_must_advertise_the_tools_capability_used_by_the_fixture() {
    let gateway = MockGateway::start_without_tools_capability().await;

    let report = run_gateway_compliance(&config(&gateway, Some(WRONG_TOKEN)))
        .await
        .expect("missing-capability mock suite should run");

    let result = case(&report, "protocol.capability-negotiation");
    assert_eq!(result.status, GatewayCaseStatus::Failed);
    assert!(result.detail.contains("tools capability"));
    let evidence = result
        .failure_evidence
        .as_ref()
        .expect("validation failure should retain initialize exchange evidence");
    assert!(matches!(
        evidence.request,
        GatewayRequestEvidence::Captured { .. }
    ));
    assert!(matches!(
        evidence.response,
        GatewayResponseEvidence::Captured { .. }
    ));
}

#[tokio::test]
async fn gateway_case_catalog_rejects_an_unmapped_specification_version() {
    let gateway = MockGateway::start(false).await;
    let unsupported = GatewayComplianceConfig {
        protocol_version: "2099-01-01",
        ..config(&gateway, Some(WRONG_TOKEN))
    };

    let error = run_gateway_compliance(&unsupported)
        .await
        .expect_err("unmapped gateway specification must be rejected");

    assert!(error.to_string().contains("support MCP 2025-11-25"));
    assert!(error.to_string().contains("requested 2099-01-01"));
    assert!(
        gateway.paths().is_empty(),
        "validation must happen before I/O"
    );
}

#[tokio::test]
async fn initialize_failure_still_emits_stable_blocked_case_rows() {
    let gateway = MockGateway::start(true).await;

    let report = run_gateway_compliance(&config(&gateway, Some(WRONG_TOKEN)))
        .await
        .expect("suite should report an initialize failure");

    for name in [
        "protocol.initialize-result",
        "protocol.version-negotiation",
        "protocol.capability-negotiation",
        "protocol.server-info",
        "protocol.initialized-notification",
        "protocol.ping",
        "session.reuse",
        "preservation.tools-list",
        "transport.invalid-protocol-version",
        "session.delete",
        "session.deleted-session",
    ] {
        assert_eq!(
            case(&report, name).status,
            GatewayCaseStatus::FixtureFailure,
            "{name} should be explicitly blocked"
        );
    }
    assert_eq!(
        report.cases.len(),
        33,
        "blocked catalog size must stay stable"
    );
}

#[tokio::test]
async fn transport_failure_records_captured_request_and_explicitly_unavailable_response() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("unused local address should bind");
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("unused listener should have an address")
    );
    drop(listener);
    let config = GatewayComplianceConfig {
        mode: StackMode::Dataplane,
        base_url: &base_url,
        server_id: "virtual/server",
        bearer_token: TOKEN,
        wrong_scope_token: Some(WRONG_TOKEN),
        protocol_version: SPEC_VERSION,
    };

    let report = run_gateway_compliance(&config)
        .await
        .expect("transport failure should be represented in the report");
    let result = case(&report, "protocol.initialize");
    assert_eq!(result.status, GatewayCaseStatus::Failed);
    let evidence = result
        .failure_evidence
        .as_ref()
        .expect("failed initialize should include structured evidence");
    assert!(matches!(
        evidence.request,
        GatewayRequestEvidence::Captured { .. }
    ));
    match &evidence.response {
        GatewayResponseEvidence::Unavailable { reason } => {
            assert!(reason.contains("before an HTTP response was received"));
        }
        GatewayResponseEvidence::Captured { .. }
        | GatewayResponseEvidence::HeadersCaptured { .. } => {
            panic!("connection failure must not fabricate an HTTP response")
        }
    }
    let json = serde_json::to_value(result).expect("failed case should serialize");
    assert_eq!(
        json.pointer("/failure_evidence/response/status"),
        Some(&Value::Null)
    );
    assert!(
        json.pointer("/failure_evidence/response/unavailable_reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| !reason.is_empty())
    );
    let markdown = render_gateway_report(&report);
    assert!(markdown.contains("Response status"));
    assert!(markdown.contains("unavailable:"));
}

#[test]
fn report_rendering_uses_the_selected_version_and_writes_round_trip_artifacts() {
    let report = GatewayComplianceReport {
        mode: "dataplane|unsafe".to_owned(),
        specification_version: "2099-01-01".to_owned(),
        cases: vec![GatewayCaseResult {
            name: "case.z".to_owned(),
            category: "Gateway".to_owned(),
            status: GatewayCaseStatus::NotApplicable,
            specification: "https://example.test/spec".to_owned(),
            detail: "line one\nline | <two>".to_owned(),
            failure_evidence: None,
        }],
    };

    let rendered = render_gateway_report(&report);
    assert!(rendered.contains("MCP 2099-01-01"));
    assert!(!rendered.contains("MCP 2025-11-25"));
    assert!(rendered.contains("dataplane\\|unsafe"));
    assert!(rendered.contains("line one\\\\nline \\| &lt;two&gt;"));

    let directory = tempfile::tempdir().expect("temporary report directory should exist");
    let markdown_path = directory.path().join("nested/report.md");
    let json_path = directory.path().join("nested/report.json");
    write_gateway_reports(&markdown_path, &json_path, &report)
        .expect("gateway reports should be written");
    let decoded: GatewayComplianceReport =
        serde_json::from_slice(&std::fs::read(json_path).expect("JSON report should be readable"))
            .expect("JSON report should round trip");
    assert_eq!(decoded, report);
    assert_eq!(
        std::fs::read_to_string(markdown_path).expect("Markdown report should be readable"),
        rendered
    );
}

#[test]
fn report_rendering_ends_with_exactly_one_newline() {
    let report = GatewayComplianceReport {
        mode: "dataplane".to_owned(),
        specification_version: SPEC_VERSION.to_owned(),
        cases: vec![GatewayCaseResult {
            name: "transport.example".to_owned(),
            category: "HTTP transport".to_owned(),
            status: GatewayCaseStatus::Failed,
            specification: "https://example.test/spec".to_owned(),
            detail: "unexpected response".to_owned(),
            failure_evidence: None,
        }],
    };

    let rendered = render_gateway_report(&report);

    assert!(rendered.ends_with('\n'));
    assert!(!rendered.ends_with("\n\n"));
}

#[test]
fn failed_case_evidence_round_trips_and_renders_machine_readable_fields() {
    let report = GatewayComplianceReport {
        mode: "dataplane".to_owned(),
        specification_version: SPEC_VERSION.to_owned(),
        cases: vec![GatewayCaseResult {
            name: "security.example".to_owned(),
            category: "Security".to_owned(),
            status: GatewayCaseStatus::Failed,
            specification: "https://example.test/spec".to_owned(),
            detail: "unexpected response".to_owned(),
            failure_evidence: Some(GatewayFailureEvidence {
                stack_mode: "dataplane".to_owned(),
                protocol_version: SPEC_VERSION.to_owned(),
                request: GatewayRequestEvidence::Captured {
                    method: "POST".to_owned(),
                    url: "https://gateway.example/servers/test/mcp".to_owned(),
                    headers: [("authorization".to_owned(), "<redacted>".to_owned())]
                        .into_iter()
                        .collect(),
                    body: Some("{\"jsonrpc\":\"2.0\"}".to_owned()),
                },
                response: GatewayResponseEvidence::Captured {
                    status: 200,
                    headers: [("content-type".to_owned(), "application/json".to_owned())]
                        .into_iter()
                        .collect(),
                    body: "{\"result\":{}}".to_owned(),
                },
            }),
        }],
    };

    let json = serde_json::to_value(&report).expect("failure evidence should serialize");
    assert_eq!(
        json.pointer("/cases/0/failure_evidence/request/availability"),
        Some(&Value::String("captured".to_owned()))
    );
    assert_eq!(
        json.pointer("/cases/0/failure_evidence/response/status"),
        Some(&Value::from(200))
    );
    let decoded: GatewayComplianceReport =
        serde_json::from_value(json).expect("failure evidence should deserialize");
    assert_eq!(decoded, report);

    let markdown = render_gateway_report(&report);
    assert!(markdown.contains("## Failure evidence"));
    assert!(markdown.contains("Request method"));
    assert!(markdown.contains("POST"));
    assert!(markdown.contains("Response status"));
    assert!(markdown.contains("200"));
    assert!(!markdown.contains("Bearer"));
}

#[test]
fn unavailable_evidence_keeps_a_stable_machine_readable_shape() {
    let evidence = GatewayFailureEvidence {
        stack_mode: "controlplane".to_owned(),
        protocol_version: SPEC_VERSION.to_owned(),
        request: GatewayRequestEvidence::Unavailable {
            reason: "request was not issued".to_owned(),
        },
        response: GatewayResponseEvidence::Unavailable {
            reason: "response was not received".to_owned(),
        },
    };

    let json = serde_json::to_value(evidence).expect("unavailable evidence should serialize");
    for pointer in [
        "/request/method",
        "/request/url",
        "/request/headers",
        "/request/body",
        "/response/status",
        "/response/headers",
        "/response/body",
    ] {
        assert_eq!(json.pointer(pointer), Some(&Value::Null), "{pointer}");
    }
    assert_eq!(
        json.pointer("/request/unavailable_reason"),
        Some(&Value::String("request was not issued".to_owned()))
    );
    assert_eq!(
        json.pointer("/response/unavailable_reason"),
        Some(&Value::String("response was not received".to_owned()))
    );
}
