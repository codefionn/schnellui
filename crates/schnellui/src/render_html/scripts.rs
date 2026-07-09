#[cfg(not(target_arch = "wasm32"))]
use super::*;

/// Defines `globalThis.__schnelluiPatch` (see `reconcile.js`). Run once per
/// page; updates then call the function with the new markup.
pub(crate) const RECONCILE_SCRIPT: &str = include_str!("reconcile.js");

/// The function `RECONCILE_SCRIPT` installs.
pub(crate) const PATCH_FUNCTION: &str = "__schnelluiPatch";

#[derive(Serialize)]
#[cfg(not(target_arch = "wasm32"))]
struct JsTarget<'a> {
    role: &'static str,
    name: Option<&'a str>,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn drive_script(action: &DriveAction) -> String {
    let (target, operation, value) = match action {
        DriveAction::Click(target) => (target, "click", None),
        DriveAction::SetValue(target, value) => (target, "set", Some(value.as_str())),
        DriveAction::Increment(target) => (target, "increment", None),
        DriveAction::Decrement(target) => (target, "decrement", None),
    };
    let target = serde_json::to_string(&JsTarget {
        role: target.role.dom_name(),
        name: target.name.as_deref(),
    })
    .expect("serializable target");
    let operation = serde_json::to_string(operation).expect("serializable operation");
    let value = serde_json::to_string(&value).expect("serializable value");
    format!(
        r#"(() => {{
const target = {target};
const element = [...document.querySelectorAll('[data-sui-role]')].find(
  node => node.dataset.suiRole === target.role &&
    (target.name === null || node.dataset.suiName === target.name)
);
if (!element) return {{ found: false, callback: false }};
const operation = {operation};
let callback = false;
if (operation === 'click') {{
  callback = element.hasAttribute('onclick') || element.hasAttribute('onchange');
  element.click();
}}
else {{
  callback = element.hasAttribute('oninput') || element.hasAttribute('onchange');
  if (operation === 'set') element.value = {value} ?? '';
  else {{
    const step = Number(element.step || 1);
    element.value = String(Number(element.value || 0) + (operation === 'increment' ? step : -step));
  }}
  element.dispatchEvent(new Event('input', {{ bubbles: true }}));
  element.dispatchEvent(new Event('change', {{ bubbles: true }}));
}}
return {{ found: true, callback }};
}})()"#
    )
}

/// The per-update call: a constant function declaration applied to the markup
/// and stylesheet as CDP call arguments (`Runtime.callFunctionOn`). The markup
/// travels as a value, never embedded in script text, so the page parses a
/// constant-size script whatever the document holds.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn patch_call_params(
    body: &str,
    style: Option<&str>,
) -> chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams {
    use chromiumoxide::cdp::js_protocol::runtime::{CallArgument, CallFunctionOnParams};
    let argument = |value: serde_json::Value| CallArgument::builder().value(value).build();
    CallFunctionOnParams::builder()
        .function_declaration(format!(
            "(body, style) => globalThis.{PATCH_FUNCTION}(body, style)"
        ))
        .argument(argument(serde_json::json!(body)))
        .argument(argument(serde_json::json!(style)))
        .build()
        .expect("function declaration is set")
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub(crate) struct ConfigError(pub(crate) String);

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl std::error::Error for ConfigError {}

#[cfg(not(target_arch = "wasm32"))]
static NEXT_BROWSER_PROFILE: AtomicU64 = AtomicU64::new(0);

/// Chromiumoxide otherwise reuses one process-global temp profile. A unique
/// directory prevents stale lock files or concurrent screenshot calls from
/// attaching to the wrong browser process.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct BrowserProfile(PathBuf);

#[cfg(not(target_arch = "wasm32"))]
impl BrowserProfile {
    pub(crate) fn create() -> std::io::Result<Self> {
        loop {
            let sequence = NEXT_BROWSER_PROFILE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("schnellui-html-{}-{sequence}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for BrowserProfile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
