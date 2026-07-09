//! URL routing for native SSR and browser CSR.
//!
//! Authorization is intentionally not modeled as middleware. Every directly
//! registered native route satisfies [`SsrRoute`], which has
//! [`SsrAuthorize`] as a supertrait, and the router calls `authorize` inline in
//! the same dispatch branch that calls `render`.
//!
//! Request paths are canonicalized once, before matching and therefore before
//! any authorization decision: segments are percent-decoded, and requests whose
//! decoding is ambiguous (an invalid escape, a decoded path separator, or a
//! `.`/`..` segment) match nothing. A [`RouteMatch`]'s parameters and
//! [`path`](RouteMatch::path) are thus the same values any later resource lookup
//! should observe, so authorization cannot be decided on one form of the URL
//! while the resource is looked up in another.

use std::marker::PhantomData;

use schnellui::template::{Template, Text};

use crate::render_html::{HtmlDocument, HtmlRenderer};

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
    ///
    /// This is the raw URL as it arrived; [`path`](Self::path) and
    /// [`param`](Self::param) return canonicalized values instead.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// The matched URL path without a query string or fragment.
    ///
    /// Canonicalized like the parameters: percent-decoded, with empty segments
    /// removed.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The raw query string without `?`, never decoded.
    ///
    /// Query content does not take part in matching or authorization; parse it
    /// yourself before using it for a security decision.
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// Looks up a `:parameter` or terminal `*wildcard` captured by the pattern.
    ///
    /// Values are percent-decoded, so they are the same strings a resource
    /// lookup should use.
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
    ///
    /// `reason` is rendered into the response body. Keep it free of detail that
    /// the requester must not learn (which resource exists, who owns it); prefer
    /// one message per route over per-resource diagnostics.
    pub fn deny(reason: impl Into<String>) -> Self {
        Self::Denied {
            status: 403,
            reason: reason.into(),
        }
    }

    /// Denies the route with an explicit status.
    ///
    /// `reason` is rendered into the response body; see [`deny`](Self::deny).
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
/// use schnellui::render_html::{HtmlDocument, HtmlRenderer, RouteMatch, SsrRoute};
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
///     ) -> Result<HtmlDocument, schnellui::render_html::HydrationError> {
///         Ok(renderer.render(schnellui::template::Text::new("unsafe")))
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
    ///
    /// Registration order is dispatch priority, so a pattern that an earlier
    /// pattern already covers is refused: it could never dispatch, and its
    /// authorization policy would silently never run.
    ///
    /// # Panics
    ///
    /// Panics when `pattern` is covered by an already-registered pattern.
    pub fn route<Route>(mut self, pattern: impl Into<String>, component: Route) -> Self
    where
        Route: SsrRoute<Context>,
    {
        let pattern = RoutePattern::parse(&pattern.into());
        ensure_unshadowed(self.routes.iter().map(|route| &route.pattern), &pattern);
        self.routes.push(RegisteredSsrRoute {
            pattern,
            component: Box::new(component),
        });
        self
    }

    /// Matches, authorizes, and renders one server request.
    ///
    /// A location that cannot be canonicalized is treated as unmatched (404),
    /// before any component runs.
    pub fn render(
        &self,
        location: impl Into<String>,
        context: &Context,
    ) -> Result<RouteResponse, HydrationError> {
        let location = location.into();
        if let Some(request) = request_path(&location) {
            for registered in &self.routes {
                let Some(route_match) = match_route(&registered.pattern, &request) else {
                    continue;
                };

                // Authorization deliberately lives directly beside rendering. Do
                // not extract this into middleware: this is the enforced path.
                return match registered.component.authorize(context, &route_match) {
                    Authorization::Allowed => Ok(RouteResponse {
                        status: 200,
                        document: registered.component.render(
                            &self.renderer,
                            context,
                            &route_match,
                        )?,
                    }),
                    Authorization::Denied { status, reason } => Ok(RouteResponse {
                        status,
                        document: self.renderer.render(Text::new(reason)),
                    }),
                };
            }
        }

        Ok(RouteResponse {
            status: 404,
            document: self.renderer.render(Text::new("Not found")),
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct RegisteredSsrRoute<Context> {
    pattern: RoutePattern,
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
    ///
    /// Registration order is dispatch priority, and a pattern an earlier
    /// pattern already covers is refused exactly as on the SSR side.
    pub fn route<Route>(mut self, pattern: impl Into<String>, component: Route) -> Self
    where
        Route: CsrRoute,
    {
        let pattern = RoutePattern::parse(&pattern.into());
        ensure_unshadowed(self.routes.iter().map(|route| &route.pattern), &pattern);
        self.routes.push(RegisteredCsrRoute {
            pattern,
            component: Box::new(CsrRouteAdapter(component)),
        });
        self
    }

    /// Mounts the current browser route and keeps it synchronized with internal
    /// link clicks and browser back/forward navigation.
    pub fn mount(self) -> Result<crate::render_html::WasmMount, wasm_bindgen::JsValue> {
        use std::cell::RefCell;
        use std::rc::Rc;

        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsValue;

        let location = Rc::new(RefCell::new(browser_location()?));
        let factory_location = Rc::clone(&location);
        let mut routes = self.routes;
        let factory = Box::new(move |renderer: &HtmlRenderer| {
            let location = factory_location.borrow();
            if let Some(request) = request_path(&location) {
                for registered in &mut routes {
                    let Some(route_match) = match_route(&registered.pattern, &request) else {
                        continue;
                    };
                    return registered.component.render(renderer, &route_match);
                }
            }
            renderer.render_page(Text::new("Not found"))
        });
        let mut mount = self.renderer.mount_rendered_factory(factory)?;

        let callback_location = Rc::clone(&location);
        let callback_session = Rc::clone(&mount.session);
        let callback = Closure::<dyn FnMut(String)>::new(move |next: String| {
            *callback_location.borrow_mut() = next;
            if let Err(error) =
                crate::render_html::renderer::wasm_rerender_session(&callback_session)
            {
                crate::render_html::renderer::wasm_console_error(&error);
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
    pattern: RoutePattern,
    component: Box<dyn ErasedCsrRoute>,
}

#[cfg(target_arch = "wasm32")]
trait ErasedCsrRoute {
    fn render(
        &mut self,
        renderer: &HtmlRenderer,
        route: &RouteMatch,
    ) -> crate::render_html::RenderedPage;
}

#[cfg(target_arch = "wasm32")]
struct CsrRouteAdapter<Route>(Route);

#[cfg(target_arch = "wasm32")]
impl<Route: CsrRoute> ErasedCsrRoute for CsrRouteAdapter<Route> {
    fn render(
        &mut self,
        renderer: &HtmlRenderer,
        route: &RouteMatch,
    ) -> crate::render_html::RenderedPage {
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

/// A route pattern split into segments once, when the route is registered,
/// so matching a request does no parsing of the pattern.
struct RoutePattern {
    /// The pattern as written, kept for diagnostics.
    source: String,
    /// `None` for a malformed pattern (an unnamed `:` or `*` parameter, or a
    /// wildcard that is not last), which matches nothing.
    segments: Option<Vec<PatternSegment>>,
}

enum PatternSegment {
    Literal(String),
    Param(String),
    /// Captures the rest of the path; always the last segment.
    Wildcard(String),
}

/// A request location split and canonicalized once per dispatch.
struct RequestPath {
    /// The location exactly as it arrived.
    location: String,
    /// Percent-decoded, empty segments removed.
    path: String,
    /// Raw, never decoded.
    query: Option<String>,
    /// Percent-decoded segments; empty ones removed.
    segments: Vec<String>,
}

#[cfg(test)]
thread_local! {
    static PATTERNS_PARSED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl RoutePattern {
    fn parse(pattern: &str) -> Self {
        #[cfg(test)]
        PATTERNS_PARSED.with(|count| count.set(count.get() + 1));
        let source = pattern.to_string();
        let parts = segments(pattern);
        let mut parsed = Vec::with_capacity(parts.len());
        for (index, part) in parts.iter().enumerate() {
            if let Some(name) = part.strip_prefix('*') {
                if name.is_empty() || index + 1 != parts.len() {
                    return Self {
                        source,
                        segments: None,
                    };
                }
                parsed.push(PatternSegment::Wildcard(name.to_string()));
            } else if let Some(name) = part.strip_prefix(':') {
                if name.is_empty() {
                    return Self {
                        source,
                        segments: None,
                    };
                }
                parsed.push(PatternSegment::Param(name.to_string()));
            } else {
                parsed.push(PatternSegment::Literal((*part).to_string()));
            }
        }
        Self {
            source,
            segments: Some(parsed),
        }
    }

    /// True when every request matched by `other` is also matched by `self`.
    fn covers(&self, other: &Self) -> bool {
        let (Some(mine), Some(theirs)) = (&self.segments, &other.segments) else {
            // A malformed pattern matches nothing, so it shadows nothing.
            return false;
        };
        let trailing_wildcard = |segments: &[PatternSegment]| {
            matches!(segments.last(), Some(PatternSegment::Wildcard(_)))
        };
        let own_fixed = mine.len() - usize::from(trailing_wildcard(mine));
        if trailing_wildcard(mine) {
            // The wildcard swallows any remainder, so `other` is covered as
            // long as its own leading segments are. `other`'s own wildcard (or
            // its whole length) must leave room for every position checked.
            let room = if trailing_wildcard(theirs) {
                theirs.len() - 1
            } else {
                theirs.len()
            };
            if own_fixed > room {
                return false;
            }
        } else if trailing_wildcard(theirs) || mine.len() != theirs.len() {
            // Without a wildcard `self` matches exactly one segment count, so
            // it can never cover `other`'s unbounded remainder.
            return false;
        }
        (0..own_fixed).all(|index| segment_covers(&mine[index], &theirs[index]))
    }
}

/// True when every segment `theirs` can match at one position is also matched
/// by `mine` at that position.
fn segment_covers(mine: &PatternSegment, theirs: &PatternSegment) -> bool {
    match (mine, theirs) {
        (PatternSegment::Param(_), _) => true,
        (PatternSegment::Literal(mine), PatternSegment::Literal(theirs)) => mine == theirs,
        // A fixed segment can never cover a position another pattern reads as
        // a remainder, and a positional mismatch cannot cover a parameter.
        (PatternSegment::Literal(_), PatternSegment::Param(_) | PatternSegment::Wildcard(_)) => {
            false
        }
        (PatternSegment::Wildcard(_), _) => true,
    }
}

/// Refuses a pattern that could never dispatch because an earlier pattern
/// already matches everything it matches. Without this, registering a route
/// (and with it a mandatory authorization policy) could silently never run
/// while an earlier, possibly more permissive component answers instead.
fn ensure_unshadowed<'pattern>(
    earlier: impl IntoIterator<Item = &'pattern RoutePattern>,
    candidate: &RoutePattern,
) {
    if let Some(shadowing) = earlier
        .into_iter()
        .find(|pattern| pattern.covers(candidate))
    {
        panic!(
            "route pattern {:?} is unreachable: the earlier pattern {:?} matches every request it matches",
            candidate.source, shadowing.source
        );
    }
}

/// Splits a request location into canonical segments once per dispatch.
///
/// Returns `None` when the path cannot be canonicalized unambiguously: an
/// invalid percent-escape, a decoding that is not UTF-8, a decoded path
/// separator inside a segment, or a `.`/`..` segment. Such a request matches
/// no route, so a route's authorization decision and any resource lookup that
/// later consumes [`RouteMatch`] always observe the same form of the URL.
fn request_path(location: &str) -> Option<RequestPath> {
    let (raw_path, query) = split_location(location);
    let mut segments = Vec::new();
    for raw in raw_path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        segments.push(decode_segment(raw)?);
    }
    Some(RequestPath {
        location: location.to_string(),
        path: if segments.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", segments.join("/"))
        },
        query: query.map(str::to_string),
        segments,
    })
}

/// Percent-decodes one path segment, rejecting values whose meaning would
/// change under a later decode or path-normalization step.
fn decode_segment(segment: &str) -> Option<String> {
    let decoded = if segment.contains('%') {
        let raw = segment.as_bytes();
        let mut decoded = Vec::with_capacity(raw.len());
        let mut at = 0;
        while at < raw.len() {
            if raw[at] == b'%' {
                let digits = raw.get(at + 1..at + 3)?;
                decoded.push(hex_digit(digits[0])? << 4 | hex_digit(digits[1])?);
                at += 3;
            } else {
                decoded.push(raw[at]);
                at += 1;
            }
        }
        String::from_utf8(decoded).ok()?
    } else {
        segment.to_string()
    };
    let unambiguous = decoded != "."
        && decoded != ".."
        && !decoded.contains(['/', '\\'])
        && !decoded.chars().any(char::is_control);
    unambiguous.then_some(decoded)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn match_route(pattern: &RoutePattern, request: &RequestPath) -> Option<RouteMatch> {
    let pattern_segments = pattern.segments.as_ref()?;
    let path_segments = &request.segments;
    let mut params = Vec::new();
    let mut path_index = 0;

    for pattern_segment in pattern_segments {
        match pattern_segment {
            PatternSegment::Wildcard(name) => {
                params.push((name.clone(), path_segments[path_index..].join("/")));
                path_index = path_segments.len();
                break;
            }
            PatternSegment::Param(name) => {
                let path_segment = path_segments.get(path_index)?;
                params.push((name.clone(), path_segment.clone()));
            }
            PatternSegment::Literal(literal) => {
                if path_segments.get(path_index)? != literal {
                    return None;
                }
            }
        }
        path_index += 1;
    }

    if path_index != path_segments.len() {
        return None;
    }

    Some(RouteMatch {
        location: request.location.clone(),
        path: request.path.clone(),
        query: request.query.clone(),
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
    use crate::render_html::HydrationKey;

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

    fn matches(pattern: &str, location: &str) -> Option<RouteMatch> {
        let request = request_path(location)?;
        match_route(&RoutePattern::parse(pattern), &request)
    }

    #[test]
    fn route_patterns_capture_parameters_and_terminal_wildcards() {
        let matched = matches(
            "/projects/:project/files/*path",
            "/projects/sui/files/a/b.rs?q=1",
        )
        .unwrap();

        assert_eq!(matched.path(), "/projects/sui/files/a/b.rs");
        assert_eq!(matched.query(), Some("q=1"));
        assert_eq!(matched.param("project"), Some("sui"));
        assert_eq!(matched.param("path"), Some("a/b.rs"));
        // A wildcard also matches an empty remainder.
        assert_eq!(
            matches("/files/*rest", "/files").unwrap().param("rest"),
            Some("")
        );
    }

    #[test]
    fn root_pattern_only_matches_the_root() {
        assert!(matches("/", "/").is_some());
        assert!(matches("/", "/other").is_none());
    }

    #[test]
    fn literals_and_lengths_must_match() {
        assert!(matches("/a/:id", "/a/1").is_some());
        assert!(matches("/a/:id", "/b/1").is_none());
        assert!(matches("/a/:id", "/a").is_none());
        assert!(matches("/a/:id", "/a/1/2").is_none());
        assert!(matches("/a/", "/a#frag").is_some());
    }

    #[test]
    fn malformed_patterns_match_nothing() {
        assert!(matches("/a/:", "/a/x").is_none());
        assert!(matches("/a/*", "/a/x").is_none());
        assert!(matches("/*rest/tail", "/x/tail").is_none());
    }

    #[test]
    fn requests_are_percent_decoded_before_matching() {
        let matched = matches("/users/:id", "/users/%34%32?tab=x").unwrap();
        assert_eq!(matched.param("id"), Some("42"));
        assert_eq!(matched.path(), "/users/42");
        // The raw location and query survive for logging and parsing.
        assert_eq!(matched.location(), "/users/%34%32?tab=x");
        assert_eq!(matched.query(), Some("tab=x"));
    }

    #[test]
    fn ambiguous_request_encodings_match_nothing() {
        // Invalid escapes and non-UTF-8 decodings.
        assert!(matches("/users/:id", "/users/%3").is_none());
        assert!(matches("/users/:id", "/users/%zz").is_none());
        assert!(matches("/users/:id", "/users/%ff").is_none());
        // A decoded separator would change the segment structure.
        assert!(matches("/files/*rest", "/files/a%2Fb").is_none());
        assert!(matches("/files/*rest", "/files/a%5Cb").is_none());
        // Dot segments would change meaning under path normalization.
        assert!(matches("/files/*rest", "/files/%2e%2e/secret").is_none());
        assert!(matches("/files/*rest", "/files/./x").is_none());
    }

    #[test]
    fn decoded_captures_describe_one_canonical_resource() {
        let decoded = matches("/files/*rest", "/files/a/b%2Ers").unwrap();
        let plain = matches("/files/*rest", "/files/a/b.rs").unwrap();
        assert_eq!(decoded.param("rest"), plain.param("rest"));
        assert_eq!(decoded.path(), plain.path());
    }

    #[test]
    fn coverage_analysis_only_finds_real_shadowing() {
        let covers = |earlier: &str, later: &str| {
            RoutePattern::parse(earlier).covers(&RoutePattern::parse(later))
        };
        assert!(covers("/accounts/:id", "/accounts/42"));
        assert!(covers("/accounts/:id", "/accounts/:id"));
        assert!(covers("/*rest", "/accounts/:id"));
        assert!(covers("/*rest", "/"));
        assert!(covers("/a/*rest", "/a/:id/security"));
        assert!(!covers("/accounts/:id", "/*rest"));
        assert!(!covers("/accounts/:id", "/accounts/:id/security"));
        assert!(!covers("/a/b", "/a/:id"));
        assert!(!covers("/a/:id", "/a/b/c"));
    }

    #[test]
    #[should_panic(expected = "is unreachable")]
    fn a_registered_wildcard_refuses_routes_it_would_swallow() {
        let _ = HtmlRouter::<RequestContext>::new(HtmlRenderer::new(320, 180))
            .route(
                "/*rest",
                AccountRoute {
                    renders: Arc::default(),
                },
            )
            .route(
                "/accounts/:id",
                AccountRoute {
                    renders: Arc::default(),
                },
            );
    }

    #[test]
    #[should_panic(expected = "is unreachable")]
    fn duplicate_patterns_are_refused_at_registration() {
        let _ = HtmlRouter::<RequestContext>::new(HtmlRenderer::new(320, 180))
            .route(
                "/accounts/:id",
                AccountRoute {
                    renders: Arc::default(),
                },
            )
            .route(
                "/accounts/:id",
                AccountRoute {
                    renders: Arc::default(),
                },
            );
    }

    #[test]
    fn a_trailing_wildcard_registered_last_still_dispatches() {
        struct StaticRoute;

        impl SsrAuthorize<RequestContext> for StaticRoute {
            fn authorize(&self, _: &RequestContext, _: &RouteMatch) -> Authorization {
                Authorization::allow()
            }
        }

        impl SsrRoute<RequestContext> for StaticRoute {
            fn render(
                &self,
                renderer: &HtmlRenderer,
                _: &RequestContext,
                route: &RouteMatch,
            ) -> Result<HtmlDocument, HydrationError> {
                Ok(renderer.render(Text::new(format!("seen {}", route.path()))))
            }
        }

        let router = HtmlRouter::<RequestContext>::new(HtmlRenderer::new(320, 180))
            .route(
                "/accounts/:id",
                AccountRoute {
                    renders: Arc::default(),
                },
            )
            .route("/*rest", StaticRoute);
        let signed_in = RequestContext { signed_in: true };

        let account = router.render("/accounts/42", &signed_in).unwrap();
        assert_eq!(account.status(), 200);
        assert!(account.document().as_str().contains("Account 42"));

        let other = router.render("/other", &signed_in).unwrap();
        assert_eq!(other.status(), 200);
        assert!(other.document().as_str().contains("seen /other"));
    }

    #[test]
    fn patterns_are_parsed_once_at_registration() {
        PATTERNS_PARSED.with(|count| count.set(0));
        let router = HtmlRouter::<RequestContext>::new(HtmlRenderer::new(320, 180))
            .route(
                "/accounts/:id",
                AccountRoute {
                    renders: Arc::default(),
                },
            )
            .route(
                "/other/*rest",
                AccountRoute {
                    renders: Arc::default(),
                },
            );
        assert_eq!(PATTERNS_PARSED.with(std::cell::Cell::get), 2);
        for id in 0..20 {
            let response = router
                .render(
                    format!("/accounts/{id}"),
                    &RequestContext { signed_in: true },
                )
                .unwrap();
            assert_eq!(response.status(), 200);
        }
        router
            .render("/missing", &RequestContext { signed_in: true })
            .unwrap();
        assert_eq!(PATTERNS_PARSED.with(std::cell::Cell::get), 2);
    }
}
