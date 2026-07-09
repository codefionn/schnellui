//! Shared-session browser primitives and the optional Servo 0.4 adapter.
//!
//! One [`Browser`] owns one engine instance and any number of page tabs. This is
//! the same boundary as a conventional browser profile: tabs have independent
//! navigation state while cookies and other engine site data are shared.

mod input;
mod persistence;

#[cfg(feature = "servo-engine")]
pub mod servo_engine;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub use input::{
    BrowserInput, BrowserKeyEvent, BrowserModifiers, BrowserMouseButton, BrowserPointerEvent,
    BrowserPointerKind, BrowserWheelDelta,
};
pub use persistence::{BrowserStateStore, StateStoreError};
use schnellui::widgets::CursorIcon;
use serde::{Deserialize, Serialize};
use url::Url;

pub const BROWSER_STATE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BrowserTabId(String);

impl BrowserTabId {
    pub fn new(value: impl Into<String>) -> Result<Self, BrowserError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(BrowserError::InvalidTabId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowserTabState {
    pub id: BrowserTabId,
    pub url: Url,
    pub title: String,
    #[serde(default)]
    pub history: Vec<Url>,
    #[serde(default)]
    pub history_index: usize,
    #[serde(default = "default_zoom")]
    pub zoom: f32,
    #[serde(default)]
    pub scroll_x: f64,
    #[serde(default)]
    pub scroll_y: f64,
    /// Full root-document height in CSS pixels. Hosts can use this to provide
    /// native scrollbar chrome while Servo does not paint classic scrollbars.
    #[serde(default)]
    pub content_height: f64,
    /// Visible root viewport height in CSS pixels.
    #[serde(default)]
    pub viewport_height: f64,
}

const fn default_zoom() -> f32 {
    1.0
}

impl BrowserTabState {
    pub fn new(id: BrowserTabId, url: Url) -> Self {
        Self {
            id,
            title: url.host_str().unwrap_or(url.as_str()).to_owned(),
            history: vec![url.clone()],
            history_index: 0,
            url,
            zoom: 1.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            content_height: 0.0,
            viewport_height: 0.0,
        }
    }

    fn navigated(&mut self, url: Url) {
        self.history.truncate(self.history_index.saturating_add(1));
        if self.history.last() != Some(&url) {
            self.history.push(url.clone());
            self.history_index = self.history.len() - 1;
        }
        self.url = url;
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        self.content_height = 0.0;
        self.viewport_height = 0.0;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedCookie {
    /// URL used as the cookie's origin when restoring it through Servo.
    pub origin: Url,
    /// RFC 6265 Set-Cookie representation, including attributes.
    pub set_cookie: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowserSessionState {
    pub schema_version: u32,
    pub active_tab: Option<BrowserTabId>,
    #[serde(default)]
    pub tabs: Vec<BrowserTabState>,
    #[serde(default)]
    pub cookies: Vec<PersistedCookie>,
}

impl Default for BrowserSessionState {
    fn default() -> Self {
        Self {
            schema_version: BROWSER_STATE_SCHEMA_VERSION,
            active_tab: None,
            tabs: Vec::new(),
            cookies: Vec::new(),
        }
    }
}

impl BrowserSessionState {
    pub fn validate(&self) -> Result<(), BrowserError> {
        if self.schema_version != BROWSER_STATE_SCHEMA_VERSION {
            return Err(BrowserError::UnsupportedSchema(self.schema_version));
        }
        let mut ids = std::collections::BTreeSet::new();
        for tab in &self.tabs {
            if !ids.insert(&tab.id) {
                return Err(BrowserError::DuplicateTab(tab.id.clone()));
            }
            if tab.history.is_empty() || tab.history_index >= tab.history.len() {
                return Err(BrowserError::InvalidHistory(tab.id.clone()));
            }
            if !tab.zoom.is_finite() || !(0.1..=10.0).contains(&tab.zoom) {
                return Err(BrowserError::InvalidZoom(tab.id.clone()));
            }
            if !tab.scroll_x.is_finite()
                || !tab.scroll_y.is_finite()
                || !tab.content_height.is_finite()
                || !tab.viewport_height.is_finite()
                || tab.scroll_x < 0.0
                || tab.scroll_y < 0.0
                || tab.content_height < 0.0
                || tab.viewport_height < 0.0
            {
                return Err(BrowserError::InvalidScrollGeometry(tab.id.clone()));
            }
        }
        if let Some(active) = &self.active_tab {
            if !ids.contains(active) {
                return Err(BrowserError::UnknownTab(active.clone()));
            }
        }
        Ok(())
    }
}

pub trait BrowserEngine {
    type TabHandle;
    type Error: std::error::Error + Send + Sync + 'static;

    fn open_tab(&mut self, state: &BrowserTabState) -> Result<Self::TabHandle, Self::Error>;
    fn close_tab(&mut self, handle: Self::TabHandle);
    fn set_active(&mut self, handle: &Self::TabHandle, active: bool);
    fn navigate(&mut self, handle: &Self::TabHandle, url: &Url);
    fn go_back(&mut self, handle: &Self::TabHandle, restored_target: &Url);
    fn go_forward(&mut self, handle: &Self::TabHandle, restored_target: &Url);
    fn reload(&mut self, handle: &Self::TabHandle);
    fn set_zoom(&mut self, handle: &Self::TabHandle, zoom: f32);
    fn dispatch_input(&mut self, handle: &Self::TabHandle, input: BrowserInput);
    fn spin_event_loop(&mut self);

    /// Current pointer cursor requested by the embedded content.
    fn cursor(&self, _handle: &Self::TabHandle) -> CursorIcon {
        CursorIcon::Default
    }

    fn sync_state(&mut self, _handle: &Self::TabHandle, _state: &mut BrowserTabState) {}

    fn render(&mut self, _handle: &Self::TabHandle) -> Option<BrowserFrame> {
        None
    }

    fn restore_cookies(&mut self, _cookies: &[PersistedCookie]) -> Result<(), Self::Error> {
        Ok(())
    }

    fn snapshot_cookies(&mut self, _origins: &[Url]) -> Result<Vec<PersistedCookie>, Self::Error> {
        Ok(Vec::new())
    }
}

struct LiveTab<H> {
    state: BrowserTabState,
    handle: H,
}

/// Multi-tab browser controller with a single shared engine/profile.
pub struct Browser<E: BrowserEngine> {
    engine: E,
    tabs: BTreeMap<BrowserTabId, LiveTab<E::TabHandle>>,
    active: Option<BrowserTabId>,
    metrics: BrowserPerformanceMetrics,
}

impl<E: BrowserEngine> Browser<E> {
    pub fn new(engine: E) -> Self {
        Self {
            engine,
            tabs: BTreeMap::new(),
            active: None,
            metrics: BrowserPerformanceMetrics::default(),
        }
    }

    pub fn restore(mut engine: E, state: BrowserSessionState) -> Result<Self, BrowserError> {
        state.validate()?;
        engine
            .restore_cookies(&state.cookies)
            .map_err(|error| BrowserError::Engine(error.to_string()))?;
        let mut browser = Self::new(engine);
        for tab in state.tabs {
            browser.open(tab)?;
        }
        if let Some(active) = state.active_tab {
            browser.activate(&active)?;
        }
        Ok(browser)
    }

    pub fn open(&mut self, state: BrowserTabState) -> Result<(), BrowserError> {
        if self.tabs.contains_key(&state.id) {
            return Err(BrowserError::DuplicateTab(state.id));
        }
        let handle = self
            .engine
            .open_tab(&state)
            .map_err(|error| BrowserError::Engine(error.to_string()))?;
        let id = state.id.clone();
        self.tabs.insert(id.clone(), LiveTab { state, handle });
        self.activate(&id)
    }

    pub fn close(&mut self, id: &BrowserTabId) -> Result<BrowserTabState, BrowserError> {
        let live = self
            .tabs
            .remove(id)
            .ok_or_else(|| BrowserError::UnknownTab(id.clone()))?;
        self.engine.close_tab(live.handle);
        if self.active.as_ref() == Some(id) {
            self.active = self.tabs.keys().next().cloned();
            if let Some(next) = &self.active {
                self.engine.set_active(&self.tabs[next].handle, true);
            }
        }
        Ok(live.state)
    }

    pub fn activate(&mut self, id: &BrowserTabId) -> Result<(), BrowserError> {
        if !self.tabs.contains_key(id) {
            return Err(BrowserError::UnknownTab(id.clone()));
        }
        if let Some(previous) = &self.active {
            if previous != id {
                self.engine.set_active(&self.tabs[previous].handle, false);
            }
        }
        self.engine.set_active(&self.tabs[id].handle, true);
        self.active = Some(id.clone());
        Ok(())
    }

    pub fn navigate(&mut self, id: &BrowserTabId, url: Url) -> Result<(), BrowserError> {
        let live = self
            .tabs
            .get_mut(id)
            .ok_or_else(|| BrowserError::UnknownTab(id.clone()))?;
        self.engine.navigate(&live.handle, &url);
        live.state.navigated(url);
        Ok(())
    }

    pub fn go_back(&mut self, id: &BrowserTabId) -> Result<(), BrowserError> {
        let live = self
            .tabs
            .get_mut(id)
            .ok_or_else(|| BrowserError::UnknownTab(id.clone()))?;
        if live.state.history_index > 0 {
            live.state.history_index -= 1;
            live.state.url = live.state.history[live.state.history_index].clone();
            self.engine.go_back(&live.handle, &live.state.url);
        }
        Ok(())
    }

    pub fn go_forward(&mut self, id: &BrowserTabId) -> Result<(), BrowserError> {
        let live = self
            .tabs
            .get_mut(id)
            .ok_or_else(|| BrowserError::UnknownTab(id.clone()))?;
        if live.state.history_index + 1 < live.state.history.len() {
            live.state.history_index += 1;
            live.state.url = live.state.history[live.state.history_index].clone();
            self.engine.go_forward(&live.handle, &live.state.url);
        }
        Ok(())
    }

    pub fn reload(&mut self, id: &BrowserTabId) -> Result<(), BrowserError> {
        let live = self
            .tabs
            .get(id)
            .ok_or_else(|| BrowserError::UnknownTab(id.clone()))?;
        self.engine.reload(&live.handle);
        Ok(())
    }

    pub fn set_zoom(&mut self, id: &BrowserTabId, zoom: f32) -> Result<(), BrowserError> {
        if !zoom.is_finite() || !(0.1..=10.0).contains(&zoom) {
            return Err(BrowserError::InvalidZoom(id.clone()));
        }
        let live = self
            .tabs
            .get_mut(id)
            .ok_or_else(|| BrowserError::UnknownTab(id.clone()))?;
        live.state.zoom = zoom;
        self.engine.set_zoom(&live.handle, zoom);
        Ok(())
    }

    pub fn dispatch_input(
        &mut self,
        id: &BrowserTabId,
        input: BrowserInput,
    ) -> Result<(), BrowserError> {
        let started = Instant::now();
        let live = self
            .tabs
            .get(id)
            .ok_or_else(|| BrowserError::UnknownTab(id.clone()))?;
        self.engine.dispatch_input(&live.handle, input);
        self.metrics.record_input(started.elapsed());
        Ok(())
    }

    pub fn spin_event_loop(&mut self) {
        let started = Instant::now();
        self.engine.spin_event_loop();
        for live in self.tabs.values_mut() {
            self.engine.sync_state(&live.handle, &mut live.state);
        }
        self.metrics.record_spin(started.elapsed());
    }

    pub fn render(&mut self, id: &BrowserTabId) -> Result<Option<BrowserFrame>, BrowserError> {
        let started = Instant::now();
        let live = self
            .tabs
            .get(id)
            .ok_or_else(|| BrowserError::UnknownTab(id.clone()))?;
        let frame = self.engine.render(&live.handle);
        self.metrics.record_render(started.elapsed());
        Ok(frame)
    }

    pub fn tab(&self, id: &BrowserTabId) -> Option<&BrowserTabState> {
        self.tabs.get(id).map(|live| &live.state)
    }

    pub fn active_tab(&self) -> Option<&BrowserTabId> {
        self.active.as_ref()
    }

    /// Current content cursor for one browser tab.
    pub fn cursor(&self, id: &BrowserTabId) -> Result<CursorIcon, BrowserError> {
        let live = self
            .tabs
            .get(id)
            .ok_or_else(|| BrowserError::UnknownTab(id.clone()))?;
        Ok(self.engine.cursor(&live.handle))
    }

    pub fn metrics(&self) -> BrowserPerformanceMetrics {
        self.metrics
    }

    /// Runs an engine-specific operation against one tab without exposing the
    /// controller's internal handle map. Intended for advanced embedding hooks
    /// such as JavaScript evaluation and accessibility grafting.
    pub fn with_engine_tab<R>(
        &mut self,
        id: &BrowserTabId,
        operation: impl FnOnce(&mut E, &E::TabHandle) -> R,
    ) -> Result<R, BrowserError> {
        let live = self
            .tabs
            .get(id)
            .ok_or_else(|| BrowserError::UnknownTab(id.clone()))?;
        Ok(operation(&mut self.engine, &live.handle))
    }

    pub fn snapshot(&mut self) -> Result<BrowserSessionState, BrowserError> {
        for live in self.tabs.values_mut() {
            self.engine.sync_state(&live.handle, &mut live.state);
        }
        let origins: Vec<_> = self
            .tabs
            .values()
            .flat_map(|live| {
                std::iter::once(live.state.url.clone()).chain(live.state.history.iter().cloned())
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let cookies = self
            .engine
            .snapshot_cookies(&origins)
            .map_err(|error| BrowserError::Engine(error.to_string()))?;
        Ok(BrowserSessionState {
            schema_version: BROWSER_STATE_SCHEMA_VERSION,
            active_tab: self.active.clone(),
            tabs: self.tabs.values().map(|live| live.state.clone()).collect(),
            cookies,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrowserPerformanceMetrics {
    pub input_samples: u64,
    pub input_total: Duration,
    pub input_max: Duration,
    pub spin_samples: u64,
    pub spin_total: Duration,
    pub spin_max: Duration,
    pub render_samples: u64,
    pub render_total: Duration,
    pub render_max: Duration,
}

impl BrowserPerformanceMetrics {
    fn record_input(&mut self, duration: Duration) {
        self.input_samples += 1;
        self.input_total += duration;
        self.input_max = self.input_max.max(duration);
    }

    fn record_spin(&mut self, duration: Duration) {
        self.spin_samples += 1;
        self.spin_total += duration;
        self.spin_max = self.spin_max.max(duration);
    }

    fn record_render(&mut self, duration: Duration) {
        self.render_samples += 1;
        self.render_total += duration;
        self.render_max = self.render_max.max(duration);
    }

    pub fn mean_input_latency(self) -> Duration {
        divide_duration(self.input_total, self.input_samples)
    }

    pub fn mean_spin_time(self) -> Duration {
        divide_duration(self.spin_total, self.spin_samples)
    }

    pub fn mean_render_time(self) -> Duration {
        divide_duration(self.render_total, self.render_samples)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn divide_duration(duration: Duration, samples: u64) -> Duration {
    if samples == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(duration.as_secs_f64() / samples as f64)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BrowserError {
    #[error("browser tab id cannot be empty")]
    InvalidTabId,
    #[error("unsupported browser state schema {0}")]
    UnsupportedSchema(u32),
    #[error("duplicate browser tab {0:?}")]
    DuplicateTab(BrowserTabId),
    #[error("unknown browser tab {0:?}")]
    UnknownTab(BrowserTabId),
    #[error("invalid navigation history for browser tab {0:?}")]
    InvalidHistory(BrowserTabId),
    #[error("invalid zoom for browser tab {0:?}")]
    InvalidZoom(BrowserTabId),
    #[error("invalid scroll geometry for browser tab {0:?}")]
    InvalidScrollGeometry(BrowserTabId),
    #[error("browser engine failed: {0}")]
    Engine(String),
}
