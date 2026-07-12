use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{Request, Response, StatusCode};
use axum::routing::any;
use cf_integration::conformance_fixture::{
    ConformanceFixtureClient, OFFICIAL_CONFORMANCE_BACKEND_URL, OFFICIAL_CONFORMANCE_GATEWAY_NAME,
    OFFICIAL_CONFORMANCE_SERVER_ID, ProvisionedConformanceFixture,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

const TOKEN: &str = "fixture-admin-secret";
const GATEWAY_ID: &str = "gateway-new";

#[derive(Clone, Debug)]
struct ExpectedRequest {
    method: &'static str,
    path: String,
    status: StatusCode,
    response: String,
}

#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    authorization: Option<String>,
    body: Option<Value>,
}

#[derive(Clone, Default)]
struct ApiState {
    expected: Arc<Mutex<VecDeque<ExpectedRequest>>>,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
}

struct FakeApi {
    base_url: String,
    state: ApiState,
}

impl FakeApi {
    async fn start(expected: Vec<ExpectedRequest>) -> Self {
        let state = ApiState {
            expected: Arc::new(Mutex::new(expected.into())),
            captured: Arc::default(),
        };
        let app = Router::new()
            .fallback(any(fake_api_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake API");
        let address = listener.local_addr().expect("fake API address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fake API");
        });
        Self {
            base_url: format!("http://{address}"),
            state,
        }
    }

    fn requests(&self) -> Vec<CapturedRequest> {
        self.state.captured.lock().expect("captured lock").clone()
    }

    fn assert_complete(&self) {
        let remaining = self.state.expected.lock().expect("expected lock");
        assert!(remaining.is_empty(), "unconsumed requests: {remaining:?}");
    }
}

async fn fake_api_handler(State(state): State<ApiState>, request: Request<Body>) -> Response<Body> {
    let method = request.method().to_string();
    let path = request
        .uri()
        .path_and_query()
        .map_or_else(|| request.uri().path().to_owned(), ToString::to_string);
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = to_bytes(request.into_body(), 1024 * 1024)
        .await
        .expect("read request body");
    let body = if body.is_empty() {
        None
    } else {
        Some(serde_json::from_slice(&body).expect("request JSON"))
    };
    state
        .captured
        .lock()
        .expect("captured lock")
        .push(CapturedRequest {
            method: method.clone(),
            path: path.clone(),
            authorization,
            body,
        });

    let expected = state
        .expected
        .lock()
        .expect("expected lock")
        .pop_front()
        .expect("unexpected request");
    assert_eq!(method, expected.method);
    assert_eq!(path, expected.path);
    Response::builder()
        .status(expected.status)
        .header("content-type", "application/json")
        .body(Body::from(expected.response))
        .expect("fake response")
}

async fn never_responds() -> Response<Body> {
    std::future::pending().await
}

fn response(method: &'static str, path: impl Into<String>, response: Value) -> ExpectedRequest {
    ExpectedRequest {
        method,
        path: path.into(),
        status: StatusCode::OK,
        response: response.to_string(),
    }
}

fn status(method: &'static str, path: impl Into<String>, status: StatusCode) -> ExpectedRequest {
    ExpectedRequest {
        method,
        path: path.into(),
        status,
        response: json!({"detail": status.as_u16()}).to_string(),
    }
}

fn raw_response(
    method: &'static str,
    path: impl Into<String>,
    status: StatusCode,
    response: impl Into<String>,
) -> ExpectedRequest {
    ExpectedRequest {
        method,
        path: path.into(),
        status,
        response: response.into(),
    }
}

fn gateway_record(id: &str) -> Value {
    json!({
        "id": id,
        "name": OFFICIAL_CONFORMANCE_GATEWAY_NAME,
        "url": OFFICIAL_CONFORMANCE_BACKEND_URL,
        "transport": "STREAMABLEHTTP",
        "description": "Official MCP conformance fixture",
        "enabled": true,
        "reachable": true,
        "capabilities": {}
    })
}

fn catalogs(gateway_key: &str, prompt: bool) -> [Value; 3] {
    [
        json!([
            {"id":"tool-1", "name":"test_simple_text", gateway_key:GATEWAY_ID},
            {"id":"foreign-tool", "name":"test_simple_text", gateway_key:"other-gateway"}
        ]),
        json!([
            {"id":"resource-1", "uri":"test://static-text", gateway_key:GATEWAY_ID},
            {"id":"foreign-resource", "uri":"test://static-text", gateway_key:"other-gateway"}
        ]),
        if prompt {
            json!([
                {"id":"prompt-1", "name":"test_simple_prompt", gateway_key:GATEWAY_ID},
                {"id":"foreign-prompt", "name":"test_simple_prompt", gateway_key:"other-gateway"}
            ])
        } else {
            json!([])
        },
    ]
}

fn provision_prefix(gateways: Value) -> Vec<ExpectedRequest> {
    vec![
        status(
            "DELETE",
            format!("/servers/{OFFICIAL_CONFORMANCE_SERVER_ID}"),
            StatusCode::NOT_FOUND,
        ),
        response("GET", "/gateways", gateways),
    ]
}

fn append_create_and_refresh(expected: &mut Vec<ExpectedRequest>) {
    expected.push(response("POST", "/gateways", gateway_record(GATEWAY_ID)));
    expected.push(response(
        "POST",
        format!("/gateways/{GATEWAY_ID}/tools/refresh?include_resources=true&include_prompts=true"),
        json!({"gateway_id":GATEWAY_ID, "tools_added":1, "resources_added":1, "prompts_added":1}),
    ));
}

fn append_catalogs(expected: &mut Vec<ExpectedRequest>, gateway_key: &str, prompt: bool) {
    let [tools, resources, prompts] = catalogs(gateway_key, prompt);
    expected.extend([
        response("GET", "/tools", tools),
        response("GET", "/resources", resources),
        response("GET", "/prompts", prompts),
    ]);
}

fn test_client(base_url: &str) -> ConformanceFixtureClient {
    ConformanceFixtureClient::builder(base_url, TOKEN)
        .poll_interval(Duration::ZERO)
        .max_attempts(2)
        .build()
        .expect("valid fixture client")
}

#[tokio::test]
async fn provision_uses_authenticated_admin_api_in_exact_order() {
    let mut expected = provision_prefix(json!([]));
    append_create_and_refresh(&mut expected);
    append_catalogs(&mut expected, "gatewayId", true);
    expected.push(response(
        "POST",
        "/servers",
        json!({"id":OFFICIAL_CONFORMANCE_SERVER_ID, "name":"Official MCP Conformance Server"}),
    ));
    let api = FakeApi::start(expected).await;

    let fixture = test_client(&api.base_url)
        .provision(OFFICIAL_CONFORMANCE_BACKEND_URL)
        .await
        .expect("provision fixture");

    assert_eq!(
        fixture,
        ProvisionedConformanceFixture {
            gateway_id: GATEWAY_ID.to_owned(),
            server_id: OFFICIAL_CONFORMANCE_SERVER_ID.to_owned(),
        }
    );
    let requests = api.requests();
    assert!(
        requests
            .iter()
            .all(|request| request.authorization.as_deref() == Some("Bearer fixture-admin-secret"))
    );
    let gateway_body = requests[2].body.as_ref().expect("gateway body");
    assert_eq!(
        gateway_body,
        &json!({
            "name":OFFICIAL_CONFORMANCE_GATEWAY_NAME,
            "url":OFFICIAL_CONFORMANCE_BACKEND_URL,
            "transport":"STREAMABLEHTTP"
        })
    );
    let server = requests.last().expect("server request");
    assert_eq!(server.method, "POST");
    assert_eq!(server.path, "/servers");
    assert_eq!(
        server.body.as_ref().expect("server body"),
        &json!({
            "server": {
                "id": OFFICIAL_CONFORMANCE_SERVER_ID,
                "name": "Official MCP Conformance Server",
                "description": "Virtual server for the pinned official MCP conformance fixture.",
                "associated_tools": ["tool-1"],
                "associated_resources": ["resource-1"],
                "associated_prompts": ["prompt-1"]
            }
        })
    );
    api.assert_complete();
}

#[tokio::test]
async fn provision_deletes_every_stale_reserved_gateway_before_creation() {
    let mut expected = provision_prefix(json!([
        gateway_record("stale-one"),
        {"id":"unrelated", "name":"another-gateway", "url":"http://example.test/mcp", "transport":"SSE"},
        gateway_record("stale-two")
    ]));
    expected.extend([
        status("DELETE", "/gateways/stale-one", StatusCode::NO_CONTENT),
        status("DELETE", "/gateways/stale-two", StatusCode::OK),
    ]);
    append_create_and_refresh(&mut expected);
    append_catalogs(&mut expected, "gatewayId", true);
    expected.push(response(
        "POST",
        "/servers",
        json!({"id":OFFICIAL_CONFORMANCE_SERVER_ID}),
    ));
    let api = FakeApi::start(expected).await;

    test_client(&api.base_url)
        .provision(OFFICIAL_CONFORMANCE_BACKEND_URL)
        .await
        .expect("provision fixture");

    api.assert_complete();
}

#[tokio::test]
async fn provision_accepts_snake_case_gateway_ids() {
    let mut expected = provision_prefix(json!([]));
    append_create_and_refresh(&mut expected);
    append_catalogs(&mut expected, "gateway_id", true);
    expected.push(response(
        "POST",
        "/servers",
        json!({"id":OFFICIAL_CONFORMANCE_SERVER_ID}),
    ));
    let api = FakeApi::start(expected).await;

    test_client(&api.base_url)
        .provision(OFFICIAL_CONFORMANCE_BACKEND_URL)
        .await
        .expect("snake-case catalog IDs");

    let server = api.requests().pop().expect("server request");
    assert_eq!(
        server.body.expect("server body")["server"]["associated_tools"][0],
        "tool-1"
    );
    api.assert_complete();
}

#[tokio::test]
async fn provision_retries_an_incomplete_catalog_then_succeeds() {
    let mut expected = provision_prefix(json!([]));
    append_create_and_refresh(&mut expected);
    append_catalogs(&mut expected, "gatewayId", false);
    append_catalogs(&mut expected, "gatewayId", true);
    expected.push(response(
        "POST",
        "/servers",
        json!({"id":OFFICIAL_CONFORMANCE_SERVER_ID}),
    ));
    let api = FakeApi::start(expected).await;

    test_client(&api.base_url)
        .provision(OFFICIAL_CONFORMANCE_BACKEND_URL)
        .await
        .expect("second catalog attempt succeeds");

    assert_eq!(
        api.requests()
            .iter()
            .filter(|request| request.path == "/tools")
            .count(),
        2
    );
    api.assert_complete();
}

#[tokio::test]
async fn provision_reports_missing_identity_and_cleans_partial_fixture() {
    let mut expected = provision_prefix(json!([]));
    append_create_and_refresh(&mut expected);
    append_catalogs(&mut expected, "gatewayId", false);
    append_catalogs(&mut expected, "gatewayId", false);
    expected.extend([
        status(
            "DELETE",
            format!("/servers/{OFFICIAL_CONFORMANCE_SERVER_ID}"),
            StatusCode::NOT_FOUND,
        ),
        status(
            "DELETE",
            format!("/gateways/{GATEWAY_ID}"),
            StatusCode::NO_CONTENT,
        ),
    ]);
    let api = FakeApi::start(expected).await;

    let error = test_client(&api.base_url)
        .provision(OFFICIAL_CONFORMANCE_BACKEND_URL)
        .await
        .expect_err("missing prompt must fail");

    let message = format!("{error:#}");
    assert!(message.contains("test_simple_prompt"), "{message}");
    assert!(message.contains(GATEWAY_ID), "{message}");
    api.assert_complete();
}

#[tokio::test]
async fn cleanup_deletes_server_before_gateway_and_accepts_not_found() {
    let api = FakeApi::start(vec![
        status("DELETE", "/servers/server-old", StatusCode::NOT_FOUND),
        status("DELETE", "/gateways/gateway-old", StatusCode::NOT_FOUND),
    ])
    .await;
    let fixture = ProvisionedConformanceFixture {
        gateway_id: "gateway-old".to_owned(),
        server_id: "server-old".to_owned(),
    };

    test_client(&api.base_url)
        .cleanup(Some(&fixture))
        .await
        .expect("idempotent cleanup");

    api.assert_complete();
}

#[tokio::test]
async fn cleanup_rejects_non_not_found_failure() {
    let api = FakeApi::start(vec![
        status("DELETE", "/servers/server-old", StatusCode::NO_CONTENT),
        status(
            "DELETE",
            "/gateways/gateway-old",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ])
    .await;
    let fixture = ProvisionedConformanceFixture {
        gateway_id: "gateway-old".to_owned(),
        server_id: "server-old".to_owned(),
    };

    let error = test_client(&api.base_url)
        .cleanup(Some(&fixture))
        .await
        .expect_err("cleanup status must fail");

    assert!(format!("{error:#}").contains("500"));
    api.assert_complete();
}

#[tokio::test]
async fn cleanup_still_attempts_gateway_after_server_delete_failure() {
    let api = FakeApi::start(vec![
        status(
            "DELETE",
            "/servers/server-old",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        status("DELETE", "/gateways/gateway-old", StatusCode::NO_CONTENT),
    ])
    .await;
    let fixture = ProvisionedConformanceFixture {
        gateway_id: "gateway-old".to_owned(),
        server_id: "server-old".to_owned(),
    };

    let error = test_client(&api.base_url)
        .cleanup(Some(&fixture))
        .await
        .expect_err("server cleanup status must fail");

    assert!(format!("{error:#}").contains("500"));
    api.assert_complete();
}

#[tokio::test]
async fn provision_combines_primary_and_cleanup_failures_in_that_order() {
    let mut expected = provision_prefix(json!([]));
    expected.push(response("POST", "/gateways", gateway_record(GATEWAY_ID)));
    expected.push(status(
        "POST",
        format!("/gateways/{GATEWAY_ID}/tools/refresh?include_resources=true&include_prompts=true"),
        StatusCode::BAD_GATEWAY,
    ));
    expected.push(status(
        "DELETE",
        format!("/servers/{OFFICIAL_CONFORMANCE_SERVER_ID}"),
        StatusCode::NOT_FOUND,
    ));
    expected.push(status(
        "DELETE",
        format!("/gateways/{GATEWAY_ID}"),
        StatusCode::INTERNAL_SERVER_ERROR,
    ));
    let api = FakeApi::start(expected).await;

    let error = test_client(&api.base_url)
        .provision(OFFICIAL_CONFORMANCE_BACKEND_URL)
        .await
        .expect_err("primary and cleanup must fail");

    let message = format!("{error:#}");
    let primary = message.find("502").expect("primary status");
    let cleanup = message.find("cleanup").expect("cleanup diagnostic");
    let cleanup_status = message.rfind("500").expect("cleanup status");
    assert!(primary < cleanup && cleanup < cleanup_status, "{message}");
    api.assert_complete();
}

#[tokio::test]
async fn malformed_gateway_create_response_reconciles_reserved_gateway() {
    let mut expected = provision_prefix(json!([]));
    expected.extend([
        raw_response("POST", "/gateways", StatusCode::CREATED, "{"),
        response(
            "GET",
            "/gateways",
            json!([gateway_record("gateway-committed")]),
        ),
        status(
            "DELETE",
            "/gateways/gateway-committed",
            StatusCode::NO_CONTENT,
        ),
    ]);
    let api = FakeApi::start(expected).await;

    let error = test_client(&api.base_url)
        .provision(OFFICIAL_CONFORMANCE_BACKEND_URL)
        .await
        .expect_err("malformed gateway response must fail");

    assert!(format!("{error:#}").contains("invalid JSON"));
    api.assert_complete();
}

#[tokio::test]
async fn gateway_create_and_reconciliation_failures_are_combined_primary_first() {
    let mut expected = provision_prefix(json!([]));
    expected.extend([
        raw_response("POST", "/gateways", StatusCode::CREATED, "{"),
        status("GET", "/gateways", StatusCode::INTERNAL_SERVER_ERROR),
    ]);
    let api = FakeApi::start(expected).await;

    let error = test_client(&api.base_url)
        .provision(OFFICIAL_CONFORMANCE_BACKEND_URL)
        .await
        .expect_err("create and reconciliation must fail");

    let message = format!("{error:#}");
    let primary = message.find("invalid JSON").expect("primary diagnostic");
    let reconciliation = message
        .find("reconciliation")
        .expect("reconciliation diagnostic");
    let reconciliation_status = message.rfind("500").expect("reconciliation status");
    assert!(
        primary < reconciliation && reconciliation < reconciliation_status,
        "{message}"
    );
    api.assert_complete();
}

#[test]
fn builder_rejects_invalid_base_url_and_zero_attempts() {
    let invalid_url = ConformanceFixtureClient::builder("not a URL", TOKEN)
        .build()
        .expect_err("invalid URL");
    let zero_attempts = ConformanceFixtureClient::builder("http://localhost", TOKEN)
        .max_attempts(0)
        .build()
        .expect_err("zero attempts");

    assert!(format!("{invalid_url:#}").contains("base URL"));
    assert!(format!("{zero_attempts:#}").contains("max_attempts"));
}

#[test]
fn builder_accepts_only_http_origins_with_root_path() {
    for base_url in [
        "http://localhost/api",
        "http://localhost/?scope=fixture",
        "https://localhost/#fixture",
    ] {
        let error = ConformanceFixtureClient::builder(base_url, TOKEN)
            .build()
            .expect_err("non-origin base URL");
        assert!(format!("{error:#}").contains("base URL"));
    }
    ConformanceFixtureClient::builder("https://localhost/", TOKEN)
        .build()
        .expect("root HTTPS origin");
}

#[test]
fn builder_accepts_rfc6750_b64token_and_jwt_characters() {
    for token in [
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmaXh0dXJlIn0.signature_-~",
        "abcDEF012-._~+/==",
    ] {
        ConformanceFixtureClient::builder("http://localhost", token)
            .build()
            .expect("valid RFC 6750 bearer token");
    }
}

#[test]
fn builder_rejects_zero_request_timeout() {
    let error = ConformanceFixtureClient::builder("http://localhost", TOKEN)
        .request_timeout(Duration::ZERO)
        .build()
        .expect_err("zero request timeout");

    assert!(format!("{error:#}").contains("request_timeout"));
}

#[tokio::test]
async fn request_timeout_bounds_a_non_responding_admin_api() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind hanging API");
    let address = listener.local_addr().expect("hanging API address");
    tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(any(never_responds)))
            .await
            .expect("serve hanging API");
    });
    let client = ConformanceFixtureClient::builder(format!("http://{address}"), TOKEN)
        .request_timeout(Duration::from_millis(25))
        .build()
        .expect("valid fixture client");

    let error = tokio::time::timeout(Duration::from_secs(1), client.cleanup(None))
        .await
        .expect("client request timeout must bound the operation")
        .expect_err("hanging request must fail");

    let message = format!("{error:#}");
    assert!(message.contains("timed out"), "{message}");
    assert!(!message.contains(TOKEN), "{message}");
}

#[test]
fn builder_rejects_empty_whitespace_and_internal_equals_without_echoing_token() {
    for token in ["", "token with space", "abc=def"] {
        let error = ConformanceFixtureClient::builder("http://localhost", token)
            .build()
            .expect_err("malformed bearer token");
        let message = format!("{error:#}");
        if !token.is_empty() {
            assert!(!message.contains(token), "token leaked in: {message}");
        }
    }
}

#[tokio::test]
async fn token_is_absent_from_debug_and_errors() {
    let builder = ConformanceFixtureClient::builder("http://127.0.0.1:9", TOKEN);
    assert!(!format!("{builder:?}").contains(TOKEN));
    let client = builder.build().expect("valid client");
    assert!(!format!("{client:?}").contains(TOKEN));

    let error = client
        .cleanup(None)
        .await
        .expect_err("closed port must fail");

    assert!(!format!("{error:?}").contains(TOKEN));
    assert!(!format!("{error:#}").contains(TOKEN));
}

#[tokio::test]
async fn token_is_redacted_when_it_matches_a_fixture_id() {
    let token = "server-old";
    let api = FakeApi::start(vec![
        status(
            "DELETE",
            "/servers/server-old",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        status("DELETE", "/gateways/gateway-old", StatusCode::NO_CONTENT),
    ])
    .await;
    let client = ConformanceFixtureClient::builder(&api.base_url, token)
        .build()
        .expect("valid client");
    let fixture = ProvisionedConformanceFixture {
        gateway_id: "gateway-old".to_owned(),
        server_id: token.to_owned(),
    };

    let error = client
        .cleanup(Some(&fixture))
        .await
        .expect_err("cleanup status must fail");

    assert!(!format!("{error:#}").contains(token));
    api.assert_complete();
}
