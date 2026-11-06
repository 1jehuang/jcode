#[must_use]
pub(super) struct SharedMcpHandleLease<'a> {
    pool: &'a SharedMcpPool,
    reference_id: u64,
    name: String,
    handle: McpHandle,
    new_reference: bool,
    _admission: tokio::sync::RwLockReadGuard<'a, ()>,
}

impl SharedMcpHandleLease<'_> {
    pub(super) fn handle(&self) -> McpHandle {
        self.handle.clone()
    }

    pub(super) fn commit(mut self) {
        self.new_reference = false;
    }
}

impl Drop for SharedMcpHandleLease<'_> {
    fn drop(&mut self) {
        if self.new_reference {
            self.pool
                .release_reference_on_drop(DeferredReferenceRelease::Claim(
                    self.reference_id,
                    std::mem::take(&mut self.name),
                ));
        }
    }
}
