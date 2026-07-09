//! WASM CSR client for the native-HTML router examples.

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
    use html_native_router::{DashboardCsrRoute, APP_STATE, HEIGHT, ROUTE_PATTERN, WIDTH};
    use schnellui_render_html::{HtmlRenderer, HtmlRouter};

    let renderer = HtmlRenderer::new(WIDTH, HEIGHT);
    let state = renderer
        .take_hydration(APP_STATE)
        .map_err(|error| wasm_bindgen::JsValue::from_str(&error.to_string()))?;

    let mount = HtmlRouter::<()>::new(renderer)
        .route(ROUTE_PATTERN, DashboardCsrRoute::new(state))
        .mount()?;
    mount.forget();
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("html_native_router is the WASM CSR client; run html_native_router_ssr natively");
}

#[cfg(target_arch = "wasm32")]
fn main() {}
