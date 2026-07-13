//! Compliance artifact paths, loading, and report generation.

use super::*;

const CONFORMANCE_COMPLETION_MARKER: &[u8] = b"complete\n";

impl<R: ProcessRunner> RuntimeExecutor<R> {
    pub(super) fn regenerate_compliance_reports(
        &self,
        results_dir: Option<&Path>,
        output_dir: Option<&Path>,
    ) -> AppResult<()> {
        let paths = CompliancePaths::new(
            results_dir.unwrap_or_else(|| self.config.integration_dir()),
            output_dir
                .map(Path::to_owned)
                .unwrap_or_else(|| self.config.root().join("reports")),
        );
        let conformance_targets = [
            ConformanceTarget::Fixture,
            ConformanceTarget::Controlplane,
            ConformanceTarget::Dataplane,
        ];
        let has_conformance = conformance_targets
            .into_iter()
            .map(|target| paths.conformance_mode(target))
            .any(|artifact| artifact.metadata.is_file() || artifact.official_results.is_dir());
        if has_conformance {
            let comparison = self.write_comparison_from_artifacts(&paths, None)?;
            println!("Conformance comparison: {}", comparison.display());
        }

        let conformance_spec = conformance_targets
            .into_iter()
            .map(|target| self.load_conformance_artifact(&paths, target))
            .collect::<AppResult<Vec<_>>>()?
            .into_iter()
            .flatten()
            .map(|artifact| artifact.metadata.spec_version)
            .next();
        if conformance_spec
            .as_deref()
            .is_some_and(|version| version != COVERAGE_SPEC_VERSION)
        {
            println!(
                "Specification coverage skipped: inventory is pinned to {COVERAGE_SPEC_VERSION}, official conformance used {}",
                conformance_spec.as_deref().unwrap_or_default()
            );
        } else {
            let coverage = self.write_spec_coverage_report(&paths.report_output, &paths)?;
            println!("Specification coverage: {}", coverage.display());
        }

        for target in [BaselineTarget::Controlplane, BaselineTarget::Dataplane] {
            let Some(report) = self.load_gateway_artifact(&paths, target)? else {
                continue;
            };
            let markdown = paths
                .report_output
                .join(format!("mcp-gateway-compliance-{}.md", target_slug(target)));
            let json = paths.report_output.join(format!(
                "mcp-gateway-compliance-{}.json",
                target_slug(target)
            ));
            write_gateway_reports(&markdown, &json, &report).map_err(AppFailure::from)?;
            println!("Gateway compliance report: {}", markdown.display());
        }
        Ok(())
    }

    pub(super) fn write_spec_coverage_report(
        &self,
        output_dir: &Path,
        paths: &CompliancePaths,
    ) -> AppResult<PathBuf> {
        let checkout = self.config.integration_dir().join("mcp-spec-source");
        let request = CheckoutRequest::controlplane(
            &checkout,
            PINNED_SOURCE_REPOSITORY,
            PINNED_SOURCE_COMMIT,
        );
        self.ensure_checkout(&request)?;
        let actual_commit = self.git_required(&checkout, ["rev-parse", "HEAD"])?;
        if actual_commit != PINNED_SOURCE_COMMIT {
            return Err(AppFailure::from(anyhow!(
                "MCP specification checkout resolved to {actual_commit}, expected pinned commit {PINNED_SOURCE_COMMIT}"
            )));
        }
        let requirements = extract_catalog_from_checkout(&checkout).map_err(AppFailure::from)?;
        let overlay_path = self
            .config
            .root()
            .join("conformance/coverage-overrides.yml");
        let overlay_source = fs::read_to_string(&overlay_path)
            .with_context(|| format!("failed to read MCP coverage overlay {overlay_path:?}"))
            .map_err(AppFailure::from)?;
        let mut overlay =
            parse_coverage_overlay(&overlay_source, &requirements).map_err(AppFailure::from)?;
        self.enrich_coverage_overlay(&mut overlay, paths)?;
        let output = output_dir.join("mcp-spec-coverage.md");
        write_coverage_report(&output, &requirements, &overlay).map_err(AppFailure::from)?;
        Ok(output)
    }

    pub(super) fn enrich_coverage_overlay(
        &self,
        overlay: &mut CoverageOverlay,
        paths: &CompliancePaths,
    ) -> AppResult<()> {
        let controlplane_official =
            self.load_conformance_artifact(paths, ConformanceTarget::Controlplane)?;
        let dataplane_official =
            self.load_conformance_artifact(paths, ConformanceTarget::Dataplane)?;
        let controlplane_gateway =
            self.load_gateway_artifact(paths, BaselineTarget::Controlplane)?;
        let dataplane_gateway = self.load_gateway_artifact(paths, BaselineTarget::Dataplane)?;
        for artifact in [controlplane_official.as_ref(), dataplane_official.as_ref()]
            .into_iter()
            .flatten()
        {
            if artifact.metadata.spec_version != overlay.spec_version {
                return Err(AppFailure::from(anyhow!(
                    "official conformance artifact specification {:?} does not match coverage specification {:?}",
                    artifact.metadata.spec_version,
                    overlay.spec_version
                )));
            }
        }
        let controlplane = ModeCoverageEvidence::from_artifacts(
            controlplane_official
                .as_ref()
                .map(|artifact| &artifact.results),
            controlplane_gateway.as_ref(),
            controlplane_official
                .as_ref()
                .and_then(|artifact| artifact.metadata.fixture.as_ref()),
        );
        let dataplane = ModeCoverageEvidence::from_artifacts(
            dataplane_official
                .as_ref()
                .map(|artifact| &artifact.results),
            dataplane_gateway.as_ref(),
            dataplane_official
                .as_ref()
                .and_then(|artifact| artifact.metadata.fixture.as_ref()),
        );
        enrich_overlay_results(overlay, &controlplane, &dataplane);
        Ok(())
    }

    pub(super) fn load_gateway_artifact(
        &self,
        paths: &CompliancePaths,
        target: BaselineTarget,
    ) -> AppResult<Option<GatewayComplianceReport>> {
        let artifact = paths.gateway_mode(target);
        if !artifact.json.is_file() {
            return Ok(None);
        }
        let source = fs::read(&artifact.json)
            .with_context(|| format!("failed to read gateway artifact {:?}", artifact.json))
            .map_err(AppFailure::from)?;
        let report: GatewayComplianceReport = serde_json::from_slice(&source)
            .with_context(|| format!("failed to parse gateway artifact {:?}", artifact.json))
            .map_err(AppFailure::from)?;
        if report.mode != target_slug(target) {
            return Err(AppFailure::from(anyhow!(
                "gateway artifact mode {:?} does not match {} path",
                report.mode,
                target_slug(target)
            )));
        }
        if report.specification_version != cf_integration_compliance::coverage::SPEC_VERSION {
            return Err(AppFailure::from(anyhow!(
                "gateway artifact specification {:?} does not match coverage specification {:?}",
                report.specification_version,
                cf_integration_compliance::coverage::SPEC_VERSION
            )));
        }
        Ok(Some(report))
    }

    pub(super) fn write_comparison_from_artifacts(
        &self,
        paths: &CompliancePaths,
        expected_run: Option<(&str, &str)>,
    ) -> AppResult<PathBuf> {
        let fixture = self.load_conformance_artifact(paths, ConformanceTarget::Fixture)?;
        let controlplane =
            self.load_conformance_artifact(paths, ConformanceTarget::Controlplane)?;
        let dataplane = self.load_conformance_artifact(paths, ConformanceTarget::Dataplane)?;
        if fixture.is_none() && controlplane.is_none() && dataplane.is_none() {
            return Err(AppFailure::from(anyhow!(
                "no official conformance artifacts found beneath {}",
                paths.conformance_root.display()
            )));
        }

        let metadata = compatible_metadata(
            fixture.as_ref().map(|artifact| &artifact.metadata),
            controlplane.as_ref().map(|artifact| &artifact.metadata),
            dataplane.as_ref().map(|artifact| &artifact.metadata),
            expected_run,
        )?;
        let empty_results = ConformanceResults::default();
        let empty_baseline = Baseline::default();
        let scenarios = compare_result_sets_with_fixture_trust(
            fixture
                .as_ref()
                .map_or(&empty_results, |artifact| &artifact.results),
            controlplane
                .as_ref()
                .map_or(&empty_results, |artifact| &artifact.results),
            dataplane
                .as_ref()
                .map_or(&empty_results, |artifact| &artifact.results),
            controlplane
                .as_ref()
                .map_or(&empty_baseline, |artifact| &artifact.baseline),
            dataplane
                .as_ref()
                .map_or(&empty_baseline, |artifact| &artifact.baseline),
            ComparisonFixtureTrust {
                fixture: is_trusted_official_fixture(
                    fixture
                        .as_ref()
                        .and_then(|artifact| artifact.metadata.fixture.as_ref()),
                ),
                controlplane: is_trusted_official_fixture(
                    controlplane
                        .as_ref()
                        .and_then(|artifact| artifact.metadata.fixture.as_ref()),
                ),
                dataplane: is_trusted_official_fixture(
                    dataplane
                        .as_ref()
                        .and_then(|artifact| artifact.metadata.fixture.as_ref()),
                ),
            },
        );
        let output = paths.report_output.join("mcp-conformance-comparison.md");
        write_comparison_report(
            &output,
            &ComparisonReport {
                spec_version: metadata.spec_version.clone(),
                suite: metadata.suite.clone(),
                fixture: metadata.fixture.clone(),
                scenarios,
            },
        )
        .map_err(AppFailure::from)?;
        Ok(output)
    }

    pub(super) fn load_conformance_artifact(
        &self,
        paths: &CompliancePaths,
        target: impl Into<ConformanceTarget>,
    ) -> AppResult<Option<LoadedConformanceArtifact>> {
        let target = target.into();
        let artifact = paths.conformance_mode(target);
        if !artifact.metadata.is_file()
            && !artifact.official_results.is_dir()
            && !artifact.completion.is_file()
        {
            return Ok(None);
        }
        if !artifact.metadata.is_file()
            || !artifact.official_results.is_dir()
            || !artifact.completion.is_file()
        {
            return Err(AppFailure::from(anyhow!(
                "incomplete conformance artifacts for {target} beneath {}",
                artifact.root.display()
            )));
        }
        verify_completion_marker(&artifact.completion)?;
        let metadata = read_run_metadata(&artifact.metadata)?;
        if metadata.target != target.label() {
            return Err(AppFailure::from(anyhow!(
                "conformance metadata target {:?} does not match {target}",
                metadata.target
            )));
        }
        if metadata.oracle != cf_integration_compliance::conformance::OFFICIAL_CONFORMANCE_PACKAGE {
            return Err(AppFailure::from(anyhow!(
                "conformance artifacts used oracle {:?}, expected {:?}",
                metadata.oracle,
                cf_integration_compliance::conformance::OFFICIAL_CONFORMANCE_PACKAGE
            )));
        }
        let results = load_server_results(&artifact.official_results).map_err(AppFailure::from)?;
        validate_server_scenario_set(&results, &metadata.suite, &metadata.spec_version)
            .map_err(AppFailure::from)?;
        let baseline = match target.baseline_target() {
            Some(baseline_target) => {
                let baseline_path = if artifact.rich_baseline.is_file() {
                    artifact.rich_baseline
                } else {
                    default_baseline_path(self.config.root(), baseline_target)
                };
                load_baseline(&baseline_path, baseline_target).map_err(AppFailure::from)?
            }
            None => Baseline::default(),
        };
        Ok(Some(LoadedConformanceArtifact {
            results,
            baseline,
            metadata,
        }))
    }
}

#[derive(Debug, Clone)]
pub(super) struct CompliancePaths {
    pub(super) conformance_root: PathBuf,
    pub(super) gateway_root: PathBuf,
    pub(super) report_output: PathBuf,
}

impl CompliancePaths {
    pub(super) fn new(artifact_root: &Path, report_output: PathBuf) -> Self {
        Self {
            conformance_root: artifact_root.join("conformance"),
            gateway_root: artifact_root.join("gateway-compliance"),
            report_output,
        }
    }

    pub(super) fn conformance_mode(
        &self,
        target: impl Into<ConformanceTarget>,
    ) -> ConformanceModePaths {
        let root = self
            .conformance_root
            .join(conformance_target_slug(target.into()));
        ConformanceModePaths {
            official_results: root.join("official"),
            projection: root.join("expected-failures.yml"),
            rich_baseline: root.join("baseline.yml"),
            metadata: root.join("metadata.json"),
            completion: root.join("complete"),
            root,
        }
    }

    pub(super) fn gateway_mode(&self, target: BaselineTarget) -> GatewayModePaths {
        let root = self.gateway_root.join(target_slug(target));
        GatewayModePaths {
            markdown: root.join("report.md"),
            json: root.join("report.json"),
            root,
        }
    }

    pub(super) fn clear_selected(
        &self,
        selection: ComplianceMode,
        conformance: bool,
        gateway: bool,
    ) -> AppResult<()> {
        for mode in selected_modes(selection) {
            if conformance {
                remove_artifact_directory(&self.conformance_mode(conformance_target(mode)).root)?;
            }
            if gateway {
                remove_artifact_directory(&self.gateway_mode(baseline_target(mode)).root)?;
            }
        }
        if conformance {
            remove_artifact_directory(&self.conformance_mode(ConformanceTarget::Fixture).root)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(super) struct ConformanceModePaths {
    pub(super) root: PathBuf,
    pub(super) official_results: PathBuf,
    pub(super) projection: PathBuf,
    pub(super) rich_baseline: PathBuf,
    pub(super) metadata: PathBuf,
    pub(super) completion: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct GatewayModePaths {
    pub(super) root: PathBuf,
    pub(super) markdown: PathBuf,
    pub(super) json: PathBuf,
}

pub(super) struct LoadedConformanceArtifact {
    pub(super) results: ConformanceResults,
    pub(super) baseline: Baseline,
    pub(super) metadata: ConformanceRunMetadata,
}

pub(super) fn recreate_directory(path: &Path) -> AppResult<()> {
    if path.exists() {
        fs::remove_dir_all(path)
            .with_context(|| format!("failed to clear result directory {path:?}"))
            .map_err(AppFailure::from)?;
    }
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create result directory {path:?}"))
        .map_err(AppFailure::from)
}

pub(super) fn remove_file_if_exists(path: &Path) -> AppResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppFailure::from(
            anyhow!(error).context(format!("failed to clear completion marker {path:?}")),
        )),
    }
}

pub(super) fn write_completion_marker(path: &Path) -> AppResult<()> {
    fs::write(path, CONFORMANCE_COMPLETION_MARKER)
        .with_context(|| format!("failed to write conformance completion marker {path:?}"))
        .map_err(AppFailure::from)
}

pub(super) fn conformance_process_completed(process_result: &AppResult<()>) -> bool {
    match process_result {
        Ok(()) => true,
        Err(AppFailure::Platform(PlatformError::ChildExit { status, .. })) => {
            status.code().is_some()
        }
        Err(AppFailure::Platform(PlatformError::Native(_))) | Err(AppFailure::Native(_)) => false,
    }
}

pub(super) fn mark_conformance_complete(
    process_result: &AppResult<()>,
    results: &ConformanceResults,
    target: impl Into<ConformanceTarget>,
    suite: &str,
    spec_version: &str,
    path: &Path,
) -> AppResult<bool> {
    let target = target.into();
    if !conformance_process_completed(process_result) {
        return Ok(false);
    }
    validate_server_scenario_set(results, suite, spec_version)
        .with_context(|| format!("official conformance did not complete for {target}"))
        .map_err(AppFailure::from)?;
    write_completion_marker(path)?;
    Ok(true)
}

pub(super) fn verify_completion_marker(path: &Path) -> AppResult<()> {
    let marker = fs::read(path)
        .with_context(|| format!("failed to read conformance completion marker {path:?}"))
        .map_err(AppFailure::from)?;
    if marker != CONFORMANCE_COMPLETION_MARKER {
        return Err(AppFailure::from(anyhow!(
            "invalid conformance completion marker {path:?}"
        )));
    }
    Ok(())
}

pub(super) fn write_rich_baseline(path: &Path, baseline: &Baseline) -> AppResult<()> {
    let serialized = serde_yaml::to_string(baseline)
        .context("failed to serialize rich conformance baseline")
        .map_err(AppFailure::from)?;
    fs::write(path, serialized)
        .with_context(|| format!("failed to write rich conformance baseline {path:?}"))
        .map_err(AppFailure::from)
}

pub(super) fn write_run_metadata(path: &Path, metadata: &ConformanceRunMetadata) -> AppResult<()> {
    let serialized = serde_json::to_vec_pretty(metadata)
        .context("failed to serialize conformance run metadata")
        .map_err(AppFailure::from)?;
    fs::write(path, serialized)
        .with_context(|| format!("failed to write conformance run metadata {path:?}"))
        .map_err(AppFailure::from)
}

pub(super) fn read_run_metadata(path: &Path) -> AppResult<ConformanceRunMetadata> {
    let source = fs::read(path)
        .with_context(|| format!("failed to read conformance run metadata {path:?}"))
        .map_err(AppFailure::from)?;
    serde_json::from_slice(&source)
        .with_context(|| format!("failed to parse conformance run metadata {path:?}"))
        .map_err(AppFailure::from)
}

pub(super) fn compatible_metadata<'a>(
    fixture: Option<&'a ConformanceRunMetadata>,
    controlplane: Option<&'a ConformanceRunMetadata>,
    dataplane: Option<&'a ConformanceRunMetadata>,
    expected_run: Option<(&str, &str)>,
) -> AppResult<&'a ConformanceRunMetadata> {
    let metadata = fixture.or(controlplane).or(dataplane).ok_or_else(|| {
        AppFailure::from(anyhow!(
            "no conformance metadata is available for reporting"
        ))
    })?;
    for candidate in [fixture, controlplane, dataplane].into_iter().flatten() {
        if candidate.fixture != metadata.fixture {
            return Err(AppFailure::from(anyhow!(
                "direct fixture, control-plane, and dataplane conformance fixture provenance mismatch"
            )));
        }
        if candidate.spec_version != metadata.spec_version
            || candidate.suite != metadata.suite
            || candidate.oracle != metadata.oracle
        {
            return Err(AppFailure::from(anyhow!(
                "direct fixture, control-plane, and dataplane conformance artifacts were produced by incompatible runs"
            )));
        }
    }
    if let Some((spec_version, suite)) = expected_run
        && (metadata.spec_version != spec_version || metadata.suite != suite)
    {
        return Err(AppFailure::from(anyhow!(
            "conformance artifacts do not match requested spec version {spec_version:?} and suite {suite:?}"
        )));
    }
    Ok(metadata)
}

pub(super) fn format_baseline_audit(target: ConformanceTarget, audit: &BaselineAudit) -> String {
    let mut details = Vec::new();
    for (label, scenarios) in [
        ("unexpected failures", &audit.unexpected_failures),
        ("stale baseline entries", &audit.stale_entries),
        ("unobserved baseline entries", &audit.unobserved_entries),
    ] {
        if !scenarios.is_empty() {
            details.push(format!("{label}: {}", scenarios.join(", ")));
        }
    }
    format!(
        "conformance baseline audit failed for {target}: {}",
        details.join("; ")
    )
}

pub(super) fn default_baseline_path(root: &Path, target: BaselineTarget) -> PathBuf {
    root.join("conformance").join(match target {
        BaselineTarget::Controlplane => "baseline-controlplane.yml",
        BaselineTarget::Dataplane => "baseline-dataplane.yml",
    })
}

pub(super) const fn baseline_target(mode: StackMode) -> BaselineTarget {
    match mode {
        StackMode::Controlplane => BaselineTarget::Controlplane,
        StackMode::Dataplane => BaselineTarget::Dataplane,
    }
}

pub(super) const fn conformance_target(mode: StackMode) -> ConformanceTarget {
    match mode {
        StackMode::Controlplane => ConformanceTarget::Controlplane,
        StackMode::Dataplane => ConformanceTarget::Dataplane,
    }
}

pub(super) const fn conformance_target_slug(target: ConformanceTarget) -> &'static str {
    match target {
        ConformanceTarget::Fixture => "fixture-direct",
        ConformanceTarget::Controlplane => "controlplane",
        ConformanceTarget::Dataplane => "dataplane",
    }
}

pub(super) const fn target_slug(target: BaselineTarget) -> &'static str {
    match target {
        BaselineTarget::Controlplane => "controlplane",
        BaselineTarget::Dataplane => "dataplane",
    }
}

pub(super) fn remove_artifact_directory(path: &Path) -> AppResult<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppFailure::from(
            anyhow!(error).context(format!("failed to clear compliance artifacts {path:?}")),
        )),
    }
}
