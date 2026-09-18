use super::*;

impl AppState {
    pub async fn workspace_query(
        &self,
        request: crate::workspace::WorkspaceRequest,
    ) -> DesktopResult<serde_json::Value> {
        if self.shutdown_signal.is_cancelled() || self.operations.current().is_some() {
            return Err(crate::workspace::unavailable());
        }
        let (runtime, identity) = {
            let slot = self.core.lock().await;
            let core = slot.as_ref().ok_or_else(crate::workspace::unavailable)?;
            let runtime = core
                .config
                .runtime
                .clone()
                .ok_or_else(crate::workspace::unavailable)?;
            let identity =
                identity_from_config(&core.config).ok_or_else(crate::workspace::unavailable)?;
            (runtime, identity)
        };
        // Never keep the state mutex across an HTTP request.
        let value = crate::workspace::query(&runtime, request).await?;
        let slot = self.core.lock().await;
        let core = slot.as_ref().ok_or_else(crate::workspace::unavailable)?;
        if self.shutdown_signal.is_cancelled()
            || self.operations.current().is_some()
            || identity_from_config(&core.config).as_ref() != Some(&identity)
        {
            return Err(crate::workspace::unavailable());
        }
        Ok(value)
    }
}
