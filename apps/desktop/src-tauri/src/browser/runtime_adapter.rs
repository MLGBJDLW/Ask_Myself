use async_trait::async_trait;
use nexa_core::browser_runtime::{
    ActInBrowserTab, BrowserActor, BrowserControlOwner, BrowserObservation, BrowserRuntime,
    BrowserRuntimeResult, BrowserSession, BrowserTab, CreateBrowserSession, NavigateBrowserTab,
    ObserveBrowserTab, OpenBrowserTab,
};

use super::policy::NavigationActor;
use super::state::{BrowserActRequest, BrowserState};

fn actor(actor: BrowserActor) -> NavigationActor {
    match actor {
        BrowserActor::User => NavigationActor::User,
        BrowserActor::Agent => NavigationActor::Agent,
    }
}

#[async_trait]
impl BrowserRuntime for BrowserState {
    async fn create_session(
        &self,
        request: CreateBrowserSession,
    ) -> BrowserRuntimeResult<BrowserSession> {
        Ok(BrowserState::create_session(
            self,
            request.conversation_id,
            request.profile_id,
            request.initial_url.as_deref(),
            actor(request.actor),
            request.bounds,
        )
        .await?)
    }

    async fn list_sessions(&self) -> BrowserRuntimeResult<Vec<BrowserSession>> {
        Ok(BrowserState::list_sessions(self)?)
    }

    async fn bind_session(
        &self,
        conversation_id: &str,
    ) -> BrowserRuntimeResult<Option<BrowserSession>> {
        Ok(BrowserState::active_session(self, conversation_id)?)
    }

    async fn open_tab(&self, request: OpenBrowserTab) -> BrowserRuntimeResult<BrowserTab> {
        Ok(BrowserState::open_tab(
            self,
            &request.session_id,
            &request.url,
            actor(request.actor),
            request.bounds,
        )
        .await?)
    }

    async fn activate_tab(
        &self,
        session_id: &str,
        tab_id: &str,
    ) -> BrowserRuntimeResult<BrowserSession> {
        Ok(BrowserState::activate_tab(self, session_id, tab_id)?)
    }

    async fn navigate(&self, request: NavigateBrowserTab) -> BrowserRuntimeResult<BrowserTab> {
        Ok(BrowserState::navigate(
            self,
            &request.session_id,
            &request.tab_id,
            &request.url,
            actor(request.actor),
        )
        .await?)
    }

    async fn go_back(&self, session_id: &str, tab_id: &str) -> BrowserRuntimeResult<()> {
        Ok(BrowserState::go_back(self, session_id, tab_id)?)
    }

    async fn go_forward(&self, session_id: &str, tab_id: &str) -> BrowserRuntimeResult<()> {
        Ok(BrowserState::go_forward(self, session_id, tab_id)?)
    }

    async fn reload(&self, session_id: &str, tab_id: &str) -> BrowserRuntimeResult<()> {
        Ok(BrowserState::reload(self, session_id, tab_id)?)
    }

    async fn observe(
        &self,
        request: ObserveBrowserTab,
    ) -> BrowserRuntimeResult<BrowserObservation> {
        Ok(
            BrowserState::observe(self, &request.session_id, &request.tab_id, &request.call_id)
                .await?,
        )
    }

    async fn act(&self, request: ActInBrowserTab) -> BrowserRuntimeResult<BrowserObservation> {
        Ok(BrowserState::act(
            self,
            BrowserActRequest {
                call_id: &request.call_id,
                session_id: &request.session_id,
                tab_id: &request.tab_id,
                observation_id: &request.observation_id,
                action: &request.action,
                target_ref: request.target_ref.as_deref(),
                text: request.text.as_deref(),
                value: request.value.as_deref(),
                key: request.key.as_deref(),
                scroll_x: request.scroll_x,
                scroll_y: request.scroll_y,
            },
        )
        .await?)
    }

    async fn begin_element_pick(&self, session_id: &str, tab_id: &str) -> BrowserRuntimeResult<()> {
        BrowserState::acquire_control(self, session_id, BrowserControlOwner::User)?;
        BrowserState::eval_action(
            self,
            session_id,
            tab_id,
            "window.__NEXA_BROWSER_RUNTIME__?.beginPick('element')",
        )
        .await?;
        Ok(())
    }

    async fn end_element_pick(
        &self,
        session_id: &str,
        tab_id: &str,
    ) -> BrowserRuntimeResult<Option<serde_json::Value>> {
        let value = BrowserState::eval_action(
            self,
            session_id,
            tab_id,
            "window.__NEXA_BROWSER_RUNTIME__?.takeArtifact()",
        )
        .await?;
        Ok((!value.is_null()).then_some(value))
    }

    async fn acquire_control(
        &self,
        session_id: &str,
        owner: BrowserControlOwner,
    ) -> BrowserRuntimeResult<BrowserSession> {
        Ok(BrowserState::acquire_control(self, session_id, owner)?)
    }

    async fn release_control(&self, session_id: &str) -> BrowserRuntimeResult<BrowserSession> {
        Ok(BrowserState::release_control(self, session_id)?)
    }

    async fn close_tab(
        &self,
        session_id: &str,
        tab_id: &str,
    ) -> BrowserRuntimeResult<BrowserSession> {
        Ok(BrowserState::close_tab(self, session_id, tab_id)?)
    }

    async fn close_session(&self, session_id: &str) -> BrowserRuntimeResult<()> {
        Ok(BrowserState::close_session(self, session_id)?)
    }
}
