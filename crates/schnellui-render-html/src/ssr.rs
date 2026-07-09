//! Opt-in server rendering and client hydration.
//!
//! The chain owns arbitrary server state. Calling [`SsrChain::then`] replaces
//! that state with the result of the next server stage, so server work composes
//! without widening the renderer's interface. Nothing is sent to the browser
//! until [`SsrChain::hydrate`] explicitly selects a serializable value.

mod router;

#[cfg(target_arch = "wasm32")]
pub use router::navigate;
pub use router::{
    Authorization, CsrRoute, HtmlRouter, RouteMatch, RouteResponse, SsrAuthorize, SsrRoute,
};

use std::fmt;
use std::marker::PhantomData;

use schnellui_template::Template;
#[cfg(target_arch = "wasm32")]
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{HtmlDocument, HtmlRenderer};

const HYDRATION_SCRIPT_ID: &str = "schnellui-hydration";
const HYDRATION_VERSION: u8 = 1;

/// A shared, typed name for a server hydration payload and its client reader.
///
/// Declare the key next to the payload type and use the same constant on both
/// targets. The type parameter prevents a key from being read as a different
/// Rust type by accident.
///
/// ```
/// # #[cfg(feature = "ssr")] {
/// use schnellui_render_html::HydrationKey;
///
/// #[derive(serde::Serialize, serde::Deserialize)]
/// struct ClientState {
///     count: u32,
/// }
///
/// const APP_STATE: HydrationKey<ClientState> = HydrationKey::new("app-state");
/// # }
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct HydrationKey<T> {
    name: &'static str,
    marker: PhantomData<fn() -> T>,
}

impl<T> HydrationKey<T> {
    /// Creates a key. Names should be stable across the native and WASM builds.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            marker: PhantomData,
        }
    }

    pub const fn name(self) -> &'static str {
        self.name
    }
}

impl<T> Copy for HydrationKey<T> {}

impl<T> Clone for HydrationKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}

/// A server-rendering chain whose current value is available only on the server.
///
/// Server stages can contain credentials, connections, or request context. The
/// renderer never serializes the chain value itself; only the value returned by
/// [`hydrate`](Self::hydrate) crosses into the document.
///
/// ```
/// # #[cfg(feature = "ssr")] {
/// use schnellui_render_html::{HtmlRenderer, HydrationKey};
/// use schnellui_template::Text;
///
/// struct ServerState {
///     secret: String,
///     name: String,
/// }
///
/// #[derive(serde::Serialize, serde::Deserialize)]
/// struct ClientState {
///     name: String,
/// }
///
/// const APP_STATE: HydrationKey<ClientState> = HydrationKey::new("app-state");
///
/// let html = HtmlRenderer::new(640, 480)
///     .ssr(("database-password", "Ada"))
///     .then(|(secret, name)| ServerState {
///         secret: secret.into(),
///         name: name.into(),
///     })
///     .hydrate(APP_STATE, |server| ClientState {
///         name: server.name.clone(),
///     })
///     .unwrap()
///     .render(|server| {
///         let _use_only_on_server = &server.secret;
///         Text::new(format!("Hello, {}", server.name))
///     });
///
/// assert!(html.as_str().contains("Hello, Ada"));
/// assert!(!html.as_str().contains("database-password"));
/// # }
/// ```
pub struct SsrChain<'renderer, State> {
    renderer: &'renderer HtmlRenderer,
    state: State,
    hydration: Option<SerializedHydration>,
}

impl HtmlRenderer {
    /// Starts an opt-in SSR chain with server-only state.
    ///
    /// Use [`SsrChain::then`] for further server stages, and
    /// [`SsrChain::hydrate`] to explicitly select the subset exposed to the
    /// browser.
    pub fn ssr<State>(&self, state: State) -> SsrChain<'_, State> {
        SsrChain {
            renderer: self,
            state,
            hydration: None,
        }
    }

    /// Hydrates an SSR document and mounts its live client view.
    ///
    /// `initialize` runs once with the typed server payload. It can create
    /// signals or other client-owned state and returns the ordinary view factory
    /// used by [`HtmlRenderer::mount`]. The existing SSR DOM is reconciled in
    /// place rather than discarded.
    #[cfg(target_arch = "wasm32")]
    pub fn hydrate<State, Initialize, Factory, View>(
        &self,
        key: HydrationKey<State>,
        initialize: Initialize,
    ) -> Result<crate::WasmMount, HydrationError>
    where
        State: DeserializeOwned + 'static,
        Initialize: FnOnce(State) -> Factory,
        Factory: FnMut() -> View + 'static,
        View: Template + 'static,
    {
        let state = self.take_hydration(key)?;
        self.mount(initialize(state))
            .map_err(|error| HydrationError::Browser(format!("{error:?}")))
    }

    /// Removes and deserializes the typed SSR payload without mounting a view.
    ///
    /// This is the router-oriented form of [`hydrate`](Self::hydrate): use the
    /// returned value to initialize CSR route components, then call
    /// [`HtmlRouter::mount`].
    #[cfg(target_arch = "wasm32")]
    pub fn take_hydration<State>(&self, key: HydrationKey<State>) -> Result<State, HydrationError>
    where
        State: DeserializeOwned + 'static,
    {
        let source = format!(
            r#"(() => {{
const node = document.getElementById({script_id:?});
if (!node) return null;
const payload = node.textContent;
node.remove();
return payload;
}})()"#,
            script_id = HYDRATION_SCRIPT_ID,
        );
        let encoded = js_sys::eval(&source)
            .map_err(|error| HydrationError::Browser(format!("{error:?}")))?
            .as_string()
            .ok_or(HydrationError::MissingPayload)?;
        let envelope: HydrationEnvelope<State> =
            serde_json::from_str(&encoded).map_err(HydrationError::InvalidPayload)?;
        if envelope.version != HYDRATION_VERSION {
            return Err(HydrationError::UnsupportedVersion {
                expected: HYDRATION_VERSION,
                found: envelope.version,
            });
        }
        if envelope.key != key.name {
            return Err(HydrationError::KeyMismatch {
                expected: key.name,
                found: envelope.key,
            });
        }
        Ok(envelope.value)
    }
}

impl<'renderer, State> SsrChain<'renderer, State> {
    /// Runs the next SSR stage, passing ownership of the current server value.
    ///
    /// Returning a struct or tuple that contains earlier values keeps them
    /// available to later stages. Otherwise they are dropped and can never be
    /// included in hydration accidentally.
    pub fn then<Next>(self, stage: impl FnOnce(State) -> Next) -> SsrChain<'renderer, Next> {
        SsrChain {
            renderer: self.renderer,
            state: stage(self.state),
            hydration: self.hydration,
        }
    }

    /// Explicitly selects the client-visible subset of the current server state.
    ///
    /// This is the only operation in the chain that serializes data. Calling it
    /// again replaces the previous payload, which makes branching policy visible
    /// at the call site and guarantees one unambiguous client bootstrap value.
    pub fn hydrate<ClientState>(
        mut self,
        key: HydrationKey<ClientState>,
        select: impl FnOnce(&State) -> ClientState,
    ) -> Result<Self, HydrationError>
    where
        ClientState: Serialize,
    {
        let envelope = HydrationEnvelope {
            version: HYDRATION_VERSION,
            key: key.name.to_string(),
            value: select(&self.state),
        };
        let json = serde_json::to_string(&envelope).map_err(HydrationError::Serialize)?;
        self.hydration = Some(SerializedHydration {
            json: escape_script_json(json),
        });
        Ok(self)
    }

    /// Renders the final server view and, when configured, embeds its hydration
    /// payload immediately before `</body>`.
    pub fn render<View>(self, view: impl FnOnce(&State) -> View) -> HtmlDocument
    where
        View: Template,
    {
        let mut document = self.renderer.render(view(&self.state));
        if let Some(hydration) = self.hydration {
            insert_hydration(document.source_mut(), &hydration.json);
        }
        document
    }

    /// Returns a shared reference to the current server value.
    ///
    /// This is primarily useful when ordinary control flow needs to inspect a
    /// stage before deciding which stage to chain next.
    pub fn state(&self) -> &State {
        &self.state
    }
}

#[derive(Serialize, Deserialize)]
struct HydrationEnvelope<T> {
    version: u8,
    key: String,
    value: T,
}

struct SerializedHydration {
    json: String,
}

/// Failures while producing or consuming a typed hydration payload.
#[derive(Debug)]
pub enum HydrationError {
    Serialize(serde_json::Error),
    InvalidPayload(serde_json::Error),
    MissingPayload,
    UnsupportedVersion {
        expected: u8,
        found: u8,
    },
    KeyMismatch {
        expected: &'static str,
        found: String,
    },
    Browser(String),
}

impl fmt::Display for HydrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(error) => {
                write!(formatter, "cannot serialize hydration state: {error}")
            }
            Self::InvalidPayload(error) => {
                write!(formatter, "cannot deserialize hydration state: {error}")
            }
            Self::MissingPayload => formatter.write_str("SSR hydration payload is missing"),
            Self::UnsupportedVersion { expected, found } => write!(
                formatter,
                "unsupported hydration payload version {found}; expected {expected}"
            ),
            Self::KeyMismatch { expected, found } => write!(
                formatter,
                "hydration key mismatch: found {found:?}, expected {expected:?}"
            ),
            Self::Browser(error) => write!(formatter, "browser hydration failed: {error}"),
        }
    }
}

impl std::error::Error for HydrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) | Self::InvalidPayload(error) => Some(error),
            _ => None,
        }
    }
}

fn insert_hydration(document: &mut String, json: &str) {
    let position = document
        .rfind("</body>")
        .expect("HtmlRenderer always emits a body element");
    document.insert_str(
        position,
        &format!(r#"<script id="{HYDRATION_SCRIPT_ID}" type="application/json">{json}</script>"#),
    );
}

fn escape_script_json(json: String) -> String {
    json.replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

#[cfg(test)]
mod tests {
    use schnellui_template::Text;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct ClientState {
        greeting: String,
        count: u32,
    }

    const APP_STATE: HydrationKey<ClientState> = HydrationKey::new("app-state");

    #[test]
    fn server_stages_chain_and_pass_state_to_the_final_ssr_view() {
        let html = HtmlRenderer::new(320, 180)
            .ssr("Ada")
            .then(|name| format!("Hello, {name}"))
            .then(|greeting| (greeting, 41_u32 + 1))
            .render(|(greeting, count)| Text::new(format!("{greeting}: {count}")))
            .into_string();

        assert!(html.contains("Hello, Ada: 42"));
        assert!(!html.contains(HYDRATION_SCRIPT_ID));
    }

    #[test]
    fn only_explicitly_selected_state_is_hydrated() {
        struct ServerState {
            database_password: &'static str,
            greeting: String,
        }

        let html = HtmlRenderer::new(320, 180)
            .ssr(ServerState {
                database_password: "do-not-leak",
                greeting: "Hello from SSR".into(),
            })
            .hydrate(APP_STATE, |server| ClientState {
                greeting: server.greeting.clone(),
                count: 3,
            })
            .unwrap()
            .render(|server| {
                let _server_only_connection_secret = server.database_password;
                Text::new(server.greeting.clone())
            })
            .into_string();

        assert!(html.contains("Hello from SSR"));
        assert!(html.contains(HYDRATION_SCRIPT_ID));
        assert!(html.contains(r#""greeting":"Hello from SSR""#));
        assert!(!html.contains("do-not-leak"));
    }

    #[test]
    fn embedded_json_cannot_close_its_script_element() {
        let html = HtmlRenderer::new(320, 180)
            .ssr(())
            .hydrate(APP_STATE, |_| ClientState {
                greeting: "</script><script>alert('no')</script>".into(),
                count: 0,
            })
            .unwrap()
            .render(|_| Text::new("safe"))
            .into_string();

        assert_eq!(html.matches("<script").count(), 1);
        assert!(!html.contains("</script><script>"));
        assert!(html.contains(r#"\u003c/script\u003e"#));
    }

    #[test]
    fn hydration_envelope_round_trips_with_its_typed_key() {
        let envelope = HydrationEnvelope {
            version: HYDRATION_VERSION,
            key: APP_STATE.name().to_string(),
            value: ClientState {
                greeting: "hydrated".into(),
                count: 7,
            },
        };
        let json = serde_json::to_string(&envelope).unwrap();
        let decoded: HydrationEnvelope<ClientState> = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.version, HYDRATION_VERSION);
        assert_eq!(decoded.key, APP_STATE.name());
        assert_eq!(decoded.value, envelope.value);
    }
}
