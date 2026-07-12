use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn run_host_patch(source: &str) -> (std::process::ExitStatus, String) {
    let directory = tempfile::tempdir().expect("create patch test directory");
    let target = directory.path().join("everything-server.ts");
    fs::write(&target, source).expect("write patch test input");
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docker/patch-mcp-conformance-hosts.mjs");

    let output = Command::new("node")
        .arg(script)
        .arg(&target)
        .output()
        .expect("run conformance host patch");
    let contents = fs::read_to_string(target).expect("read patch test output");
    (output.status, contents)
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
fn conformance_fixture_is_an_explicit_overlay_and_profile() {
    let default_project = ComposeProject::dataplane(
        Path::new("/repo"),
        Path::new("/checkout"),
        OsString::from("project"),
        false,
    );

    assert!(default_project.profiles().is_empty());
    assert!(
        !default_project
            .files()
            .iter()
            .any(|file| file.ends_with("docker-compose.cf-conformance.yaml"))
    );

    let conformance = default_project
        .clone()
        .with_profiles(["testing"])
        .with_conformance_fixture(Path::new("/repo"));
    assert_eq!(conformance.profiles(), ["testing", "conformance"]);
    assert_eq!(
        &conformance.files()[..default_project.files().len()],
        default_project.files()
    );
    assert_eq!(
        conformance.files().last().map(PathBuf::as_path),
        Some(Path::new("/repo/docker/docker-compose.cf-conformance.yaml"))
    );
    let deduplicated = conformance.with_conformance_fixture(Path::new("/repo"));
    assert_eq!(deduplicated.profiles(), ["testing", "conformance"]);
    assert_eq!(
        deduplicated
            .files()
            .iter()
            .filter(|file| file.ends_with("docker-compose.cf-conformance.yaml"))
            .count(),
        1
    );
}

#[test]
fn conformance_container_inputs_pin_the_runner_revision_and_patch_only_hosts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dockerfile = fs::read_to_string(root.join("docker/mcp-conformance-server.Dockerfile"))
        .expect("read conformance Dockerfile");
    let patch = fs::read_to_string(root.join("docker/patch-mcp-conformance-hosts.mjs"))
        .expect("read host patch script");
    let compose = fs::read_to_string(root.join("docker/docker-compose.cf-conformance.yaml"))
        .expect("read conformance Compose overlay");

    assert!(dockerfile.contains("FROM node:22-bookworm-slim"));
    assert!(
        dockerfile
            .contains("ARG MCP_CONFORMANCE_REVISION=21a9a2febd7100d7c17ac1021ee7f2ed9f66a1e0")
    );
    assert!(dockerfile.contains(
        "git clone https://github.com/modelcontextprotocol/conformance.git mcp-conformance"
    ));
    assert!(
        dockerfile
            .contains("git -C mcp-conformance checkout --detach \"${MCP_CONFORMANCE_REVISION}\"")
    );
    assert!(dockerfile.contains("WORKDIR /opt/mcp-conformance/examples/servers/typescript"));
    assert!(dockerfile.contains("npm ci"));
    assert!(
        dockerfile
            .contains("node /usr/local/bin/patch-mcp-conformance-hosts.mjs everything-server.ts")
    );
    assert!(dockerfile.contains(
        "git diff --exit-code -- . ':(exclude)examples/servers/typescript/everything-server.ts'"
    ));
    assert!(
        dockerfile
            .contains("git diff --numstat -- examples/servers/typescript/everything-server.ts")
    );
    assert!(
        dockerfile
            .contains("awk 'NF == 3 && $1 == 1 && $2 == 1 { count++ } END { print count + 0 }'")
    );
    assert!(dockerfile.contains("END { print count + 0 }')\" = 1"));
    assert!(dockerfile.contains(
        "grep -Fxc \"const app = createMcpExpressApp({ allowedHosts: ['mcp_conformance_server', 'localhost', '127.0.0.1', '::1'] });\" examples/servers/typescript/everything-server.ts"
    ));
    assert!(dockerfile.contains("ENV PORT=3000"));
    assert!(dockerfile.contains("EXPOSE 3000"));
    assert!(dockerfile.contains("CMD [\"npm\", \"start\"]"));

    let old = "const app = createMcpExpressApp();";
    let replacement = "const app = createMcpExpressApp({ allowedHosts: ['mcp_conformance_server', 'localhost', '127.0.0.1', '::1'] });";
    assert!(patch.contains(old));
    assert!(patch.contains(replacement));
    assert!(patch.contains("replacementCount !== 1"));
    assert!(patch.contains("process.argv[2]"));

    let actual_compose: serde_yaml::Value =
        serde_yaml::from_str(&compose).expect("parse conformance Compose overlay");
    let expected_compose: serde_yaml::Value = serde_yaml::from_str(
        r#"
services:
  mcp_conformance_server:
    profiles: ["conformance"]
    image: cf-integration/mcp-conformance-server:0.1.16
    build:
      context: ${CF_INTEGRATION_ROOT:?Set CF_INTEGRATION_ROOT to the integration harness root}
      dockerfile: docker/mcp-conformance-server.Dockerfile
    restart: "no"
    environment:
      PORT: "3000"
    networks:
      - mcpnet
    healthcheck:
      test:
        - CMD
        - node
        - -e
        - fetch('http://127.0.0.1:3000/mcp').then(response => { if (response.status !== 400) process.exit(1); }).catch(() => process.exit(1))
      interval: 2s
      timeout: 2s
      retries: 30
      start_period: 2s
"#,
    )
    .expect("parse expected conformance Compose contract");
    assert_eq!(actual_compose, expected_compose);
}

#[test]
fn conformance_host_patch_is_fail_closed_and_rewrites_exactly_one_target() {
    let old = "const app = createMcpExpressApp();";
    let replacement = "const app = createMcpExpressApp({ allowedHosts: ['mcp_conformance_server', 'localhost', '127.0.0.1', '::1'] });";

    let (status, patched) = run_host_patch(&format!("before\n{old}\nafter\n"));
    assert!(status.success());
    assert_eq!(patched, format!("before\n{replacement}\nafter\n"));

    for unchanged in ["no patch target\n".to_owned(), format!("{old}\n{old}\n")] {
        let (status, contents) = run_host_patch(&unchanged);
        assert!(!status.success());
        assert_eq!(contents, unchanged);
    }
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
