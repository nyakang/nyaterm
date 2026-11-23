pub(crate) struct TestAppCleanup(gpui::TestAppContext);
impl TestAppCleanup {
    pub(crate) fn new(cx: &gpui::TestAppContext) -> Self {
        Self(cx.clone())
    }
}
impl Drop for TestAppCleanup {
    fn drop(&mut self) {
        self.0.quit();
    }
}
