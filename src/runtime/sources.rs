//! Source checkout synchronization.

use super::*;

impl<R: ProcessRunner> RuntimeExecutor<R> {
    pub(super) fn sync_sources(&self) -> AppResult<()> {
        self.ensure_controlplane()?;
        self.ensure_dataplane()?;
        Ok(())
    }

    pub(super) fn ensure_mode_sources(&self, mode: StackMode) -> AppResult<()> {
        self.ensure_controlplane()?;
        if mode == StackMode::Dataplane {
            self.ensure_dataplane()?;
        }
        Ok(())
    }

    pub(super) fn ensure_controlplane(&self) -> AppResult<()> {
        let request = CheckoutRequest::controlplane(
            self.config.controlplane_dir(),
            self.config.controlplane_repo().value.clone(),
            self.config.controlplane_ref().value.clone(),
        );
        self.ensure_checkout(&request)
    }

    fn ensure_dataplane(&self) -> AppResult<()> {
        let request = CheckoutRequest::dataplane(
            self.config.dataplane_dir(),
            self.config.dataplane_repo().value.clone(),
            self.config.dataplane_ref().value.clone(),
        );
        self.ensure_checkout(&request)
    }

    pub(super) fn ensure_checkout(&self, request: &CheckoutRequest) -> AppResult<()> {
        let manager = CheckoutManager::new(&self.runner);
        let mut warnings = Vec::new();
        let result = manager.ensure(self.config.integration_dir(), request, &mut warnings);
        for warning in warnings {
            eprintln!("{warning}");
        }
        Ok(result.map(|_| ())?)
    }
}
