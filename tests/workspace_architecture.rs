use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use serde_json::Value;

#[test]
fn workspace_packages_and_internal_edges_match_the_architecture() {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata should start");
    assert!(output.status.success(), "cargo metadata failed");

    let metadata: Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata should return JSON");
    let packages = metadata["packages"]
        .as_array()
        .expect("metadata packages should be an array");
    let by_id = packages
        .iter()
        .map(|package| {
            (
                package["id"].as_str().expect("package id").to_owned(),
                package["name"].as_str().expect("package name").to_owned(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let members = metadata["workspace_members"]
        .as_array()
        .expect("workspace members should be an array")
        .iter()
        .map(|id| by_id[id.as_str().expect("workspace member id")].clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        members,
        BTreeSet::from([
            "cf-integration".to_owned(),
            "cf-integration-platform".to_owned(),
            "cf-integration-mcp".to_owned(),
            "cf-integration-compliance".to_owned(),
            "cf-integration-load".to_owned(),
        ])
    );

    let workspace_names = &members;
    let edges = packages
        .iter()
        .flat_map(|package| {
            let source = package["name"].as_str().expect("package name").to_owned();
            package["dependencies"]
                .as_array()
                .expect("dependencies should be an array")
                .iter()
                .filter_map(move |dependency| {
                    let target = dependency["name"].as_str().expect("dependency name");
                    workspace_names
                        .contains(target)
                        .then(|| (source.clone(), target.to_owned()))
                })
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        edges,
        BTreeSet::from([
            (
                "cf-integration".to_owned(),
                "cf-integration-platform".to_owned(),
            ),
            ("cf-integration".to_owned(), "cf-integration-mcp".to_owned(),),
            (
                "cf-integration".to_owned(),
                "cf-integration-compliance".to_owned(),
            ),
            (
                "cf-integration".to_owned(),
                "cf-integration-load".to_owned(),
            ),
            (
                "cf-integration-compliance".to_owned(),
                "cf-integration-platform".to_owned(),
            ),
            (
                "cf-integration-compliance".to_owned(),
                "cf-integration-mcp".to_owned(),
            ),
            (
                "cf-integration-load".to_owned(),
                "cf-integration-platform".to_owned(),
            ),
            (
                "cf-integration-load".to_owned(),
                "cf-integration-mcp".to_owned(),
            ),
        ])
    );

    let binary_targets = packages
        .iter()
        .flat_map(|package| {
            let package_name = package["name"].as_str().expect("package name");
            package["targets"]
                .as_array()
                .expect("targets should be an array")
                .iter()
                .filter(|target| {
                    target["kind"]
                        .as_array()
                        .expect("target kind should be an array")
                        .iter()
                        .any(|kind| kind == "bin")
                })
                .map(move |target| {
                    (
                        package_name.to_owned(),
                        target["name"].as_str().expect("target name").to_owned(),
                    )
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        binary_targets,
        vec![("cf-integration".to_owned(), "cf-integration".to_owned())]
    );
}
