use super::*;

pub(super) struct SafariProvider;

#[async_trait]
impl BrowserProvider for SafariProvider {
    fn id(&self) -> &'static str {
        crate::browser_safari::BACKEND
    }
    fn supported_browsers(&self) -> &'static [&'static str] {
        &["safari"]
    }
    async fn status(&self, _ctx: &ToolContext) -> Result<ToolOutput> {
        let value = crate::browser_safari::status().await?;
        Ok(render_browser_output(
            "status",
            "Safari status".into(),
            value,
        ))
    }
    async fn setup(&self) -> Result<ToolOutput> {
        let value = crate::browser_safari::setup().await?;
        Ok(render_browser_output("setup", "Safari setup".into(), value))
    }
    async fn ensure_ready(&self) -> Result<Option<String>> {
        // Do not install anything or silently change permissions. Each action has
        // a bounded Safari error that explains the explicit permission steps.
        Ok(None)
    }
    async fn execute(
        &self,
        action: &str,
        input: &BrowserInput,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let (command, params, title) = bridge_request(action, input)?;
        let result = crate::browser_safari::request_for(&ctx.session_id, &command, params).await?;
        let mut output = attach_browser_metadata(
            render_browser_output(action, title, result),
            self.id(),
            "safari",
        );
        if let Some(Value::Object(metadata)) = output.metadata.as_mut() {
            metadata.insert("capabilities".into(), crate::browser_safari::capabilities());
        }
        Ok(output)
    }
}
