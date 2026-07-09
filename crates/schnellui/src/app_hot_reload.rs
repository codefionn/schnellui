//! Opt-in development template reloads. Disabled applications do no socket work.

use super::*;
use schnellui::widgets::{HotReloadServer, ReloadError, ReloadReport};
use std::net::{SocketAddr, ToSocketAddrs};

pub(crate) struct HotReloadState {
    server: HotReloadServer,
    results: Vec<Result<ReloadReport, ReloadError>>,
    previous_interval: Option<Duration>,
    polling_interval: Duration,
}

impl App {
    /// Listens for template data edits and applies them before the next layout.
    ///
    /// Use a loopback address such as `"127.0.0.1:0"`. The returned address includes
    /// the chosen port. The native host polls at most every 50 ms while enabled.
    /// Changes to Rust expressions or handlers are reported as requiring a rebuild.
    pub fn enable_hot_reload(
        &mut self,
        address: impl ToSocketAddrs,
    ) -> std::io::Result<SocketAddr> {
        let server = HotReloadServer::bind(address)?;
        let bound = server.local_addr()?;
        self.disable_hot_reload();
        let previous_interval = self.redraw_interval;
        let polling_interval = previous_interval
            .unwrap_or(Duration::from_millis(50))
            .min(Duration::from_millis(50));
        self.hot_reload = Some(HotReloadState {
            server,
            results: Vec::new(),
            previous_interval,
            polling_interval,
        });
        self.set_redraw_interval(Some(polling_interval));
        Ok(bound)
    }

    /// Stops listening and restores the host's previous redraw interval.
    pub fn disable_hot_reload(&mut self) {
        if let Some(state) = self.hot_reload.take() {
            if self.redraw_interval == Some(state.polling_interval) {
                self.set_redraw_interval(state.previous_interval);
            }
        }
    }

    /// Returns the results received since the last call, including rebuild requests.
    pub fn take_hot_reload_results(&mut self) -> Vec<Result<ReloadReport, ReloadError>> {
        self.hot_reload
            .as_mut()
            .map(|state| std::mem::take(&mut state.results))
            .unwrap_or_default()
    }

    pub(crate) fn poll_hot_reload(&mut self) {
        let Some(state) = self.hot_reload.as_mut() else {
            return;
        };
        let mut ctx = BuildCtx {
            context: self.context.clone(),
            runtime: self.widgets.clone(),
            scene: &mut self.scene,
            layout: &mut self.layout,
            text: &mut self.text,
            atlas: &mut self.atlas,
            scale: self.scale,
        };
        let results = state.server.poll(&mut ctx);
        // Bound unread development diagnostics rather than retaining every edit
        // for the lifetime of a long-running preview.
        for result in results {
            if state.results.len() == 64 {
                let _ = state.results.remove(0);
            }
            state.results.push(result);
        }
    }
}
