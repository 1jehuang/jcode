use super::*;

impl Agent {
    /// Refuse to run a model turn when this session moved to another machine
    /// or this in-memory copy is older than the transcript on disk.
    pub(crate) fn ensure_session_lease(&self) -> Result<()> {
        crate::session_accounts::ensure_bound_account_route(self.provider.as_ref(), &self.session)?;
        match self.session.migration_lease_block() {
            Some(block) => Err(anyhow::anyhow!("Session is not runnable here: {block}")),
            None => Ok(()),
        }
    }

    /// True when this copy may not run turns on this machine.
    pub(crate) fn session_lease_blocked(&self) -> bool {
        self.session.migration_lease_block().is_some()
    }

    /// True when a newer migrated transcript replaced this copy on disk.
    pub(crate) fn session_copy_is_stale(&self) -> bool {
        matches!(
            self.session.migration_lease_block(),
            Some(crate::storage::SessionLeaseBlock::StaleCopy { .. })
        )
    }
}
