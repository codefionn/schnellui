//! Shared hydration contract and CSR route for the native-HTML router examples.

use std::cell::Cell;
use std::rc::Rc;

use schnellui_render_html::{CsrRoute, HydrationKey, RouteMatch};
use schnellui_template::{column, link, Button, Template, Text};
use serde::{Deserialize, Serialize};

pub const ROUTE_PATTERN: &str = "/users/:user_id/dashboard";
pub const WIDTH: u32 = 760;
pub const HEIGHT: u32 = 420;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientState {
    pub user_name: String,
    pub initial_count: u32,
    pub inner_ssr_message: String,
}

pub const APP_STATE: HydrationKey<ClientState> = HydrationKey::new("html-native-router");

/// The hydrated client route. The separate SSR example constructs the same
/// view with server-derived state before this route takes ownership in WASM.
pub struct DashboardCsrRoute {
    state: ClientState,
    count: Rc<Cell<u32>>,
}

impl DashboardCsrRoute {
    pub fn new(state: ClientState) -> Self {
        Self {
            count: Rc::new(Cell::new(state.initial_count)),
            state,
        }
    }
}

impl CsrRoute for DashboardCsrRoute {
    fn view(&mut self, route: &RouteMatch) -> impl Template + 'static {
        dashboard_view(self.state.clone(), Rc::clone(&self.count), route)
    }
}

pub fn dashboard_view(
    state: ClientState,
    count: Rc<Cell<u32>>,
    route: &RouteMatch,
) -> impl Template + 'static {
    let increment = Rc::clone(&count);
    let path = route.path().to_string();
    let query = route.query().unwrap_or("none").to_string();

    column()
        .gap(14.0)
        .child(Text::new("Outer SSR router"))
        .child(Text::new(format!("Authorized route: {path}")))
        .child(Text::new(format!("Current query: {query}")))
        .child(
            link()
                .label("Navigate with the CSR router")
                .value("/users/42/dashboard?tab=activity"),
        )
        .child(
            column()
                .gap(8.0)
                .child(Text::new(format!("Hydrated CSR for {}", state.user_name)))
                .child(Text::dynamic(move || {
                    format!("Client count: {}", count.get())
                }))
                .child(Button::new("Increment in CSR").on_click(move || {
                    increment.set(increment.get() + 1);
                }))
                .child(
                    column()
                        .gap(4.0)
                        .child(Text::new("Nested SSR component"))
                        .child(Text::new(state.inner_ssr_message)),
                ),
        )
}
