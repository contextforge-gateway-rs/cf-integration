use std::ffi::OsString;
use std::path::{Path, PathBuf};

use cf_integration::compose::{ComposeProject, validate_integration_contract};
use serde_json::json;

const EXPECTED_IMAGE: &str = "ghcr.io/ibm/cfex-mcp-fast-time-server:latest";

fn valid_config() -> serde_json::Value {
    json!({
        "services": {
            "fast_time_server": {"image": EXPECTED_IMAGE},
            "register_fast_time": {
                "command": [
                    "wait for http://fast_time_server:9080/health",
                    "register http://fast_time_server:9080/mcp"
                ]
            },
            "fast_test_server": {
                "image": "example/modern:latest",
                "profiles": ["testing"]
            },
            "register_fast_test": {"profiles": ["testing"]}
        }
    })
}

fn messages(config: &serde_json::Value) -> Vec<String> {
    validate_integration_contract(config, EXPECTED_IMAGE)
        .into_iter()
        .map(|violation| violation.to_string())
        .collect()
}

#[test]
fn dataplane_compose_files_are_in_override_order() {
    let project = ComposeProject::dataplane(
        Path::new("/repo"),
        Path::new("/checkout"),
        OsString::from("cf-integration"),
        false,
    );

    assert_eq!(
        project.files(),
        [
            PathBuf::from("/checkout/docker-compose.yml"),
            PathBuf::from("/repo/docker/docker-compose.cf-controlplane-build-labels.yaml"),
            PathBuf::from("/repo/docker/docker-compose.cf-dataplane.yaml"),
            PathBuf::from("/repo/docker/docker-compose.cf-integration.yaml"),
        ]
    );
    assert!(project.profiles().is_empty());
    assert_eq!(
        project.command(["config", "--format", "json"]).arguments(),
        [
            "compose",
            "-p",
            "cf-integration",
            "-f",
            "/checkout/docker-compose.yml",
            "-f",
            "/repo/docker/docker-compose.cf-controlplane-build-labels.yaml",
            "-f",
            "/repo/docker/docker-compose.cf-dataplane.yaml",
            "-f",
            "/repo/docker/docker-compose.cf-integration.yaml",
            "config",
            "--format",
            "json",
        ]
        .map(OsString::from)
    );
}

#[test]
fn source_dataplane_adds_build_overlay_last() {
    let project = ComposeProject::dataplane(
        Path::new("/repo"),
        Path::new("/checkout"),
        OsString::from("project"),
        true,
    );

    assert_eq!(
        project.files().last().map(PathBuf::as_path),
        Some(Path::new(
            "/repo/docker/docker-compose.cf-dataplane-build.yaml"
        ))
    );
}

#[test]
fn controlplane_profiles_are_explicit_and_sso_is_optional() {
    let without_sso = ComposeProject::controlplane(
        Path::new("/repo"),
        Path::new("/checkout"),
        OsString::from("cp"),
        false,
    );
    let with_sso = ComposeProject::controlplane(
        Path::new("/repo"),
        Path::new("/checkout"),
        OsString::from("cp"),
        true,
    );

    assert_eq!(without_sso.profiles(), ["testing", "inspector"]);
    assert_eq!(with_sso.profiles(), ["testing", "inspector", "sso"]);
    assert_eq!(
        without_sso.files(),
        [
            PathBuf::from("/checkout/docker-compose.yml"),
            PathBuf::from("/repo/docker/docker-compose.cf-controlplane-build-labels.yaml"),
        ]
    );
}

#[test]
fn valid_contract_has_no_violations() {
    assert!(messages(&valid_config()).is_empty());
}

#[test]
fn contract_reports_missing_or_wrong_fast_time_image() {
    let mut missing = valid_config();
    missing["services"]
        .as_object_mut()
        .expect("services object")
        .remove("fast_time_server");
    assert_eq!(
        messages(&missing),
        ["fast_time_server is missing from the integration compose config"]
    );

    let mut wrong = valid_config();
    wrong["services"]["fast_time_server"]["image"] = json!("wrong/image:tag");
    assert_eq!(
        messages(&wrong),
        [format!(
            "fast_time_server image is \"wrong/image:tag\"; expected \"{EXPECTED_IMAGE}\""
        )]
    );
}

#[test]
fn fast_test_services_must_stay_behind_profiles() {
    let mut config = valid_config();
    config["services"]["fast_test_server"]
        .as_object_mut()
        .expect("service object")
        .remove("profiles");
    config["services"]["register_fast_test"]["profiles"] = json!([]);

    assert_eq!(
        messages(&config),
        [
            "fast_test_server is active in the base integration stack; keep fast-test behind an explicit profile",
            "register_fast_test is active in the base integration stack; keep fast-test behind an explicit profile",
        ]
    );
}

#[test]
fn every_legacy_image_prefix_is_rejected_in_sorted_service_order() {
    let mut config = valid_config();
    let services = config["services"].as_object_mut().expect("services object");
    services.insert(
        "zulu".to_owned(),
        json!({"image": "ghcr.io/ibm/fast-time-server:old"}),
    );
    services.insert(
        "alpha".to_owned(),
        json!({"image": "mcpgateway/fast-test-server@sha256:abc"}),
    );

    assert_eq!(
        messages(&config),
        [
            "alpha uses legacy fast-test/time image \"mcpgateway/fast-test-server@sha256:abc\"",
            "zulu uses legacy fast-test/time image \"ghcr.io/ibm/fast-time-server:old\"",
        ]
    );
}

#[test]
fn registration_must_use_health_and_streamable_http_urls() {
    let mut config = valid_config();
    config["services"]["register_fast_time"]["command"] = json!("wrong command");

    assert_eq!(
        messages(&config),
        [
            "register_fast_time does not wait for fast_time_server on port 9080",
            "register_fast_time does not register the streamable HTTP endpoint at /mcp",
        ]
    );
}

#[test]
fn malformed_rendered_config_returns_stable_diagnostics_instead_of_panicking() {
    assert_eq!(
        messages(&json!({"services": []})),
        ["rendered Compose config has no services object"]
    );
    assert_eq!(
        messages(&json!({})),
        ["rendered Compose config has no services object"]
    );
}

#[test]
fn multiple_violations_have_stable_contract_order() {
    let config = json!({
        "services": {
            "register_fast_time": {"command": []},
            "fast_test_server": {"image": "ghcr.io/ibm/fast-time-server:old"}
        }
    });

    assert_eq!(
        messages(&config),
        [
            "fast_time_server is missing from the integration compose config",
            "fast_test_server is active in the base integration stack; keep fast-test behind an explicit profile",
            "fast_test_server uses legacy fast-test/time image \"ghcr.io/ibm/fast-time-server:old\"",
            "register_fast_time does not wait for fast_time_server on port 9080",
            "register_fast_time does not register the streamable HTTP endpoint at /mcp",
        ]
    );
}
