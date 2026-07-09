//! URL routing for native SSR and browser CSR.
//!
//! Authorization is intentionally not modeled as middleware. Every directly
//! registered native route satisfies [`SsrRoute`], which has
//! [`SsrAuthorize`] as a supertrait, and the router calls `authorize` inline in
//! the same dispatch branch that calls `render`.

use std::marker::PhantomData;

use schnellui_template::{Template, Text};

use crate::{HtmlDocument, HtmlRenderer};

use super::HydrationError;

/// The URL information passed directly to route components.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteMatch {
    location: String,
    path: String,
    query: Option<String>,
    params: Vec<(String, String)>,
}

impl RouteMatch {
    /// The original path, including its query string and fragment when present.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// The matched URL path without a query string or fragment.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The raw query string without `?`.
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// Looks up a `:parameter` or terminal `*wildcard` captured by the pattern.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find_map(|(candidate, value)| (candidate == name).then_some(value.as_str()))
    }
}

/// The mandatory result of an SSR route's authorization check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Authorization {
    Allowed,
    Denied { status: u16, reason: String },
}

impl Authorization {
    pub const fn allow() -> Self {
        Self::Allowed
    }

    /// Denies the route with status 403.
    pub fn deny(reason: impl Into<String>) -> Self {
        Self::Denied {
            status: 403,
            reason: reason.into(),
        }
    }

    pub fn deny_with_status(status: u16, reason: impl Into<String>) -> Self {
        Self::Denied {
            status,
            reason: reason.into(),
        }
    }
}

/// Authorization check required from every component registered in the SSR
/// router, including routes that intentionally allow every request.
pub trait SsrAuthorize<Context> {
    fn authorize(&self, context: &Context, route: &RouteMatch) -> Authorization;
}

/// A component rendered directly by the SSR router.
///
/// The [`SsrAuthorize`] supertrait is the compile-time enforcement point: a
/// component cannot implement this interface or be passed to
/// [`HtmlRouter::route`] until it declares its authorization policy. Dispatch
/// checks that policy immediately before calling `render`; there is no
/// middleware chain or bypassable secondary path.
///
/// ```compile_fail
/// use schnellui_render_html::{HtmlDocument, HtmlRenderer, RouteMatch, SsrRoute};
///
/// struct MissingAuthorization;
///
/// // Fails because `SsrAuthorize<()>` is deliberately not implemented.
/// impl SsrRoute<()> for MissingAuthorization {
///     fn render(
///         &self,
///         renderer: &HtmlRenderer,
///         _context: &(),
///         _route: &RouteMatch,
///     ) -> Result<HtmlDocument, schnellui_render_html::HydrationError> {
///         Ok(renderer.render(schnellui_template::Text::new("unsafe")))
///     }
/// }
/// ```
pub trait SsrRoute<Context>: SsrAuthorize<Context> + Send + Sync + 'static {
    fn render(
        &self,
        renderer: &HtmlRenderer,
        context: &Context,
        route: &RouteMatch,
    ) -> Result<HtmlDocument, HydrationError>;
}

/// A component rendered directly by the CSR router.
///
/// Client authorization is deliberately not part of this interface because it
/// cannot protect server data. The corresponding SSR component still has to
/// satisfy [`SsrAuthorize`] on the server build.
pub trait CsrRoute: 'static {
    fn view(&mut self, route: &RouteMatch) -> impl Template + 'static;
}

/// Result of a native SSR route dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteResponse {
    status: u16,
    document: HtmlDocument,
}

impl RouteResponse {
    pub const fn status(&self) -> u16 {
        self.status
    }

    pub fn document(&self) -> &HtmlDocument {
        &self.document
    }

    pub fn into_document(self) -> HtmlDocument {
        self.document
    }
}

/// Native-HTML router. `route` and `mount`/`render` select their CSR or SSR
/// implementation at compile time, so one route table can be declared under
/// target-specific component implementations.
pub struct HtmlRouter<Context = ()> {
    renderer: HtmlRenderer,
    #[cfg(not(target_arch = "wasm32"))]
    routes: Vec<RegisteredSsrRoute<Context>>,
    #[cfg(target_arch = "wasm32")]
    routes: Vec<RegisteredCsrRoute>,
    context: PhantomData<fn() -> Context>,
}

impl<Context> HtmlRouter<Context> {
    pub fn new(renderer: HtmlRenderer) -> Self {
        Self {
            renderer,
            routes: Vec::new(),
            context: PhantomData,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<Context: 'static> HtmlRouter<Context> {
    /// Registers an SSR component. The `SsrRoute` bound makes an authorization
    /// implementation mandatory at the call site.
    pub fn route<Route>(mut self, pattern: impl Into<String>, component: Route) -> Self
    where
        Route: SsrRoute<Context>,
    {
        self.routes.push(RegisteredSsrRoute {
            pattern: pattern.into(),
            component: Box::new(component),
        });
        self
    }

    /// Matches, authorizes, and renders one server request.
    pub fn render(
        &self,
        location: impl Into<String>,
        context: &Context,
    ) -> Result<RouteResponse, HydrationError> {
        let location = location.into();
        for registered in &self.routes {
            let Some(route_match) = match_route(&registered.pattern, &location) else {
                continue;
            };

            // Authorization deliberately lives directly beside rendering. Do
            // not extract this into middleware: this is the enforced path.
            return match registered.component.authorize(context, &route_match) {
                Authorization::Allowed => Ok(RouteResponse {
                    status: 200,
                    document: registered
                        .component
                        .render(&self.renderer, context, &route_match)?,
                }),
                Authorization::Denied { status, reason } => Ok(RouteResponse {
                    status,
                    document: self.renderer.render(Text::new(reason)),
                }),
            };
        }

        Ok(RouteResponse {
            status: 404,
            document: self.renderer.render(Text::new("Not found")),
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct RegisteredSsrRoute<Context> {
    pattern: String,
    component: Box<dyn ErasedSsrRoute<Context>>,
}

#[cfg(not(target_arch = "wasm32"))]
trait ErasedSsrRoute<Context>: Send + Sync {
    fn authorize(&self, context: &Context, route: &RouteMatch) -> Authorization;

    fn render(
        &self,
        renderer: &HtmlRenderer,
        context: &Context,
        route: &RouteMatch,
    ) -> Result<HtmlDocument, HydrationError>;
}

#[cfg(not(target_arch = "wasm32"))]
impl<Context, Route> ErasedSsrRoute<Context> for Route
where
    Route: SsrRoute<Context>,
{
    fn authorize(&self, context: &Context, route: &RouteMatch) -> Authorization {
        SsrAuthorize::authorize(self, context, route)
    }

    fn render(
        &self,
        renderer: &HtmlRenderer,
        context: &Context,
        route: &RouteMatch,
    ) -> Result<HtmlDocument, HydrationError> {
        SsrRoute::render(self, renderer, context, route)
    }
}

#[cfg(target_arch = "wasm32")]
impl<Context: 'static> HtmlRouter<Context> {
    /// Registers a client-rendered route component.
    pub fn route<Route>(mut self, pattern: impl Into<String>, component: Route) -> Self
    where
        Route: CsrRoute,
    {
        self.routes.push(RegisteredCsrRoute {
            pattern: pattern.into(),
            component: Box::new(CsrRouteAdapter(component)),
        });
        self
    }

    /// Mounts the current browser route and keeps it synchronized with internal
    /// link clicks and browser back/forward navigation.
    pub fn mount(self) -> Result<crate::WasmMount, wasm_bindgen::JsValue> {
        use std::cell::RefCell;
        use std::rc::Rc;

        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsValue;

        let location = Rc::new(RefCell::new(browser_location()?));
        let factory_location = Rc::clone(&location);
        let mut routes = self.routes;
        let factory = Box::new(move |renderer: &HtmlRenderer| {
            let location = factory_location.borrow();
            for registered in &mut routes {
                let Some(route_match) = match_route(&registered.pattern, &location) else {
                    continue;
                };
                return registered.component.render(renderer, &route_match);
            }
            renderer.render_page(Text::new("Not found"))
        });
        let mut mount = self.renderer.mount_rendered_factory(factory)?;

        let callback_location = Rc::clone(&location);
        let callback_session = Rc::clone(&mount.session);
        let callback = Closure::<dyn FnMut(String)>::new(move |next: String| {
            *callback_location.borrow_mut() = next;
            if let Err(error) = crate::wasm_rerender_session(&callback_session) {
                crate::wasm_console_error(&error);
            }
        });
        js_sys::Reflect::set(
            &js_sys::global(),
            &JsValue::from_str(ROUTER_BINDING),
            callback.as_ref(),
        )?;
        mount.router_callback = Some(callback);
        js_sys::eval(INSTALL_ROUTER_SCRIPT)?;
        Ok(mount)
    }
}

#[cfg(target_arch = "wasm32")]
struct RegisteredCsrRoute {
    pattern: String,
    component: Box<dyn ErasedCsrRoute>,
}

#[cfg(target_arch = "wasm32")]
trait ErasedCsrRoute {
    fn render(&mut self, renderer: &HtmlRenderer, route: &RouteMatch) -> crate::RenderedPage;
}

#[cfg(target_arch = "wasm32")]
struct CsrRouteAdapter<Route>(Route);

#[cfg(target_arch = "wasm32")]
impl<Route: CsrRoute> ErasedCsrRoute for CsrRouteAdapter<Route> {
    fn render(&mut self, renderer: &HtmlRenderer, route: &RouteMatch) -> crate::RenderedPage {
        renderer.render_page(self.0.view(route))
    }
}

/// Navigates without reloading the document after a CSR router is mounted.
#[cfg(target_arch = "wasm32")]
pub fn navigate(location: &str) -> Result<(), wasm_bindgen::JsValue> {
    let location = serde_json::to_string(location).expect("a route location is serializable");
    js_sys::eval(&format!("globalThis.__schnelluiNavigate({location})"))?;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn browser_location() -> Result<String, wasm_bindgen::JsValue> {
    js_sys::eval("location.pathname + location.search + location.hash")?
        .as_string()
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("browser location is not a string"))
}

#[cfg(target_arch = "wasm32")]
const ROUTER_BINDING: &str = "__schnellui_route_event";

#[cfg(target_arch = "wasm32")]
const INSTALL_ROUTER_SCRIPT: &str = r#"(() => {
const notify = () => {
  const callback = globalThis.__schnellui_route_event;
  if (typeof callback === 'function') {
    callback(location.pathname + location.search + location.hash);
  }
};
globalThis.__schnelluiNavigate = target => {
  const url = new URL(target, location.href);
  if (url.origin !== location.origin) {
    location.href = url.href;
    return;
  }
  history.pushState(null, '', url.href);
  notify();
};
if (globalThis.__schnelluiRouterInstalled) return;
globalThis.__schnelluiRouterInstalled = true;
addEventListener('popstate', notify);
document.addEventListener('click', event => {
  if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey ||
      event.shiftKey || event.altKey) return;
  const anchor = event.target.closest?.('a[href]');
  if (!anchor || anchor.target || anchor.hasAttribute('download')) return;
  const url = new URL(anchor.href, location.href);
  if (url.origin !== location.origin) return;
  event.preventDefault();
  history.pushState(null, '', url.href);
  notify();
});
})()"#;

fn match_route(pattern: &str, location: &str) -> Option<RouteMatch> {
    let (path, query) = split_location(location);
    let pattern_segments = segments(pattern);
    let path_segments = segments(path);
    let mut params = Vec::new();
    let mut path_index = 0;

    for (pattern_index, pattern_segment) in pattern_segments.iter().enumerate() {
        if let Some(name) = pattern_segment.strip_prefix('*') {
            if name.is_empty() || pattern_index + 1 != pattern_segments.len() {
                return None;
            }
            params.push((name.to_string(), path_segments[path_index..].join("/")));
            path_index = path_segments.len();
            break;
        }

        let path_segment = path_segments.get(path_index)?;
        if let Some(name) = pattern_segment.strip_prefix(':') {
            if name.is_empty() {
                return None;
            }
            params.push((name.to_string(), (*path_segment).to_string()));
        } else if pattern_segment != path_segment {
            return None;
        }
        path_index += 1;
    }

    if path_index != path_segments.len() {
        return None;
    }

    Some(RouteMatch {
        location: location.to_string(),
        path: path.to_string(),
        query: query.map(str::to_string),
        params,
    })
}

fn split_location(location: &str) -> (&str, Option<&str>) {
    let without_fragment = location.split_once('#').map_or(location, |(path, _)| path);
    let (path, query) = without_fragment
        .split_once('?')
        .map_or((without_fragment, None), |(path, query)| {
            (path, Some(query))
        });
    (if path.is_empty() { "/" } else { path }, query)
}

fn segments(path: &str) -> Vec<&str> {
    path.trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

#[cfg(all(test, target_arch = "wasm32"))]
mod csr_compile_tests {
    use super::*;

    struct Home;

    impl CsrRoute for Home {
        fn view(&mut self, route: &RouteMatch) -> impl Template + 'static {
            Text::new(format!("CSR route: {}", route.path()))
        }
    }

    #[test]
    fn csr_routes_register_with_the_same_router_interface() {
        let _router: HtmlRouter = HtmlRouter::new(HtmlRenderer::new(320, 180)).route("/", Home);
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use serde::Serialize;

    use super::*;
    use crate::HydrationKey;

    #[derive(Clone, Copy)]
    struct RequestContext {
        signed_in: bool,
    }

    struct AccountRoute {
        renders: Arc<AtomicUsize>,
    }

    #[derive(Serialize)]
    struct ClientRouteState {
        account_id: String,
    }

    const ROUTE_STATE: HydrationKey<ClientRouteState> = HydrationKey::new("route-state");

    struct HydratedAccountRoute {
        server_secret: &'static str,
    }

    impl SsrAuthorize<RequestContext> for AccountRoute {
        fn authorize(&self, context: &RequestContext, _route: &RouteMatch) -> Authorization {
            if context.signed_in {
                Authorization::allow()
            } else {
                Authorization::deny("Sign in required")
            }
        }
    }

    impl SsrRoute<RequestContext> for AccountRoute {
        fn render(
            &self,
            renderer: &HtmlRenderer,
            _context: &RequestContext,
            route: &RouteMatch,
        ) -> Result<HtmlDocument, HydrationError> {
            self.renders.fetch_add(1, Ordering::Relaxed);
            Ok(renderer.render(Text::new(format!(
                "Account {} ({})",
                route.param("id").unwrap(),
                route.query().unwrap_or_default()
            ))))
        }
    }

    impl SsrAuthorize<RequestContext> for HydratedAccountRoute {
        fn authorize(&self, _context: &RequestContext, _route: &RouteMatch) -> Authorization {
            Authorization::allow()
        }
    }

    impl SsrRoute<RequestContext> for HydratedAccountRoute {
        fn render(
            &self,
            renderer: &HtmlRenderer,
            _context: &RequestContext,
            route: &RouteMatch,
        ) -> Result<HtmlDocument, HydrationError> {
            let account_id = route.param("id").unwrap().to_string();
            renderer
                .ssr((self.server_secret, account_id))
                .hydrate(ROUTE_STATE, |(_, account_id)| ClientRouteState {
                    account_id: account_id.clone(),
                })
                .map(|chain| {
                    chain.render(|(_, account_id)| Text::new(format!("Account {account_id}")))
                })
        }
    }

    fn account_router() -> (HtmlRouter<RequestContext>, Arc<AtomicUsize>) {
        let renders = Arc::new(AtomicUsize::new(0));
        let router = HtmlRouter::new(HtmlRenderer::new(320, 180)).route(
            "/accounts/:id",
            AccountRoute {
                renders: Arc::clone(&renders),
            },
        );
        (router, renders)
    }

    #[test]
    fn ssr_dispatch_checks_authorization_directly_before_rendering() {
        let (router, renders) = account_router();
        let denied = router
            .render(
                "/accounts/42?tab=security",
                &RequestContext { signed_in: false },
            )
            .unwrap();
        assert_eq!(denied.status(), 403);
        assert!(denied.document().as_str().contains("Sign in required"));
        assert_eq!(renders.load(Ordering::Relaxed), 0);

        let (router, renders) = account_router();
        let allowed = router
            .render(
                "/accounts/42?tab=security",
                &RequestContext { signed_in: true },
            )
            .unwrap();
        assert_eq!(allowed.status(), 200);
        assert!(allowed
            .document()
            .as_str()
            .contains("Account 42 (tab=security)"));
        assert_eq!(renders.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn unmatched_ssr_routes_return_404() {
        let (router, renders) = account_router();
        let response = router
            .render("/missing", &RequestContext { signed_in: true })
            .unwrap();

        assert_eq!(response.status(), 404);
        assert!(response.document().as_str().contains("Not found"));
        assert_eq!(renders.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn ssr_routes_can_embed_only_their_explicit_csr_state() {
        let router = HtmlRouter::new(HtmlRenderer::new(320, 180)).route(
            "/accounts/:id",
            HydratedAccountRoute {
                server_secret: "route-secret",
            },
        );
        let response = router
            .render("/accounts/42", &RequestContext { signed_in: true })
            .unwrap();
        let html = response.document().as_str();

        assert_eq!(response.status(), 200);
        assert!(html.contains("Account 42"));
        assert!(html.contains(r#""account_id":"42""#));
        assert!(!html.contains("route-secret"));
    }

    #[test]
    fn route_patterns_capture_parameters_and_terminal_wildcards() {
        let matched = match_route(
            "/projects/:project/files/*path",
            "/projects/sui/files/a/b.rs?q=1",
        )
        .unwrap();

        assert_eq!(matched.path(), "/projects/sui/files/a/b.rs");
        assert_eq!(matched.query(), Some("q=1"));
        assert_eq!(matched.param("project"), Some("sui"));
        assert_eq!(matched.param("path"), Some("a/b.rs"));
    }

    #[test]
    fn root_pattern_only_matches_the_root() {
        assert!(match_route("/", "/").is_some());
        assert!(match_route("/", "/other").is_none());
    }
}
