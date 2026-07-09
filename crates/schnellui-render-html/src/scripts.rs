use super::*;
use crate::template::role_name;

#[cfg(target_arch = "wasm32")]
pub(crate) const WASM_STYLE_SCRIPT: &str = r#"(() => {
const expectedHtml = __SCHNELLUI_EXPECTED_HTML__;
const expectedDocument = new DOMParser().parseFromString(expectedHtml, 'text/html');
const expectedStyle = expectedDocument.querySelector('style');
if (!expectedStyle) throw new Error('rendered document has no style element');
let style = document.getElementById('schnellui-native-styles');
if (!style) {
  style = document.createElement('style');
  style.id = 'schnellui-native-styles';
  document.head.appendChild(style);
}
style.textContent = expectedStyle.textContent;
if (!document.getElementById('schnellui-root')) {
  const root = document.createElement('main');
  root.id = 'schnellui-root';
  document.body.appendChild(root);
}
return true;
})()"#;

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
        role: role_name(target.role),
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

pub(crate) fn dom_diff_script(expected: &HtmlDocument) -> String {
    let expected =
        serde_json::to_string(expected.as_str()).expect("HTML document is always serializable");
    DOM_DIFF_SCRIPT.replacen("__SCHNELLUI_EXPECTED_HTML__", &expected, 1)
}

const DOM_DIFF_SCRIPT: &str = r#"(() => {
const expectedHtml = __SCHNELLUI_EXPECTED_HTML__;
const expectedDocument = new DOMParser().parseFromString(expectedHtml, 'text/html');
const expectedRoot = expectedDocument.getElementById('schnellui-root');
const currentRoot = document.getElementById('schnellui-root');
const stats = {
  matches: false,
  attributes: 0,
  text: 0,
  inserted: 0,
  removed: 0,
  replaced: 0,
  moved: 0
};
if (!expectedRoot || !currentRoot) return stats;

function nodeKey(node) {
  if (!node || node.nodeType !== Node.ELEMENT_NODE) return null;
  if (node.id) return `id:${node.id}`;
  const explicit = node.getAttribute('data-sui-key');
  if (explicit) return `key:${explicit}`;
  const role = node.getAttribute('data-sui-role');
  const name = node.getAttribute('data-sui-name');
  return role && name ? `semantic:${role}:${name}` : null;
}

function compatible(current, expected) {
  return current.nodeType === expected.nodeType &&
    (current.nodeType !== Node.ELEMENT_NODE || current.tagName === expected.tagName);
}

function cloneExpected(node) {
  return document.importNode(node, true);
}

function patchAttributes(current, expected) {
  for (const attribute of [...current.attributes]) {
    if (!expected.hasAttribute(attribute.name)) {
      current.removeAttribute(attribute.name);
      stats.attributes += 1;
    }
  }
  for (const attribute of [...expected.attributes]) {
    if (current.getAttribute(attribute.name) !== attribute.value) {
      current.setAttribute(attribute.name, attribute.value);
      stats.attributes += 1;
    }
  }
}

function patchFormState(current, expected) {
  if (current instanceof HTMLInputElement) {
    if (current.type !== 'file' && current.value !== expected.value) {
      current.value = expected.value;
    }
    current.checked = expected.checked;
    current.indeterminate = expected.indeterminate;
  } else if (current instanceof HTMLTextAreaElement) {
    if (current.value !== expected.value) current.value = expected.value;
  } else if (current instanceof HTMLSelectElement) {
    current.selectedIndex = expected.selectedIndex;
  } else if (current instanceof HTMLOptionElement) {
    current.selected = expected.selected;
  } else if (current instanceof HTMLDetailsElement) {
    current.open = expected.open;
  }
}

function patchChildren(current, expected) {
  const expectedChildren = [...expected.childNodes];
  for (let index = 0; index < expectedChildren.length; index += 1) {
    const expectedChild = expectedChildren[index];
    let currentChild = current.childNodes[index] ?? null;
    const expectedKey = nodeKey(expectedChild);

    if (expectedKey && nodeKey(currentChild) !== expectedKey) {
      const match = [...current.childNodes]
        .slice(index + 1)
        .find(candidate => nodeKey(candidate) === expectedKey);
      if (match) {
        current.insertBefore(match, currentChild);
        currentChild = match;
        stats.moved += 1;
      } else {
        current.insertBefore(cloneExpected(expectedChild), currentChild);
        stats.inserted += 1;
        continue;
      }
    }

    const currentKey = nodeKey(currentChild);
    if (!expectedKey && currentKey) {
      const neededLater = expectedChildren
        .slice(index + 1)
        .some(candidate => nodeKey(candidate) === currentKey);
      if (neededLater) {
        current.insertBefore(cloneExpected(expectedChild), currentChild);
        stats.inserted += 1;
        continue;
      }
    }

    if (!currentChild) {
      current.appendChild(cloneExpected(expectedChild));
      stats.inserted += 1;
      continue;
    }

    if (!compatible(currentChild, expectedChild)) {
      current.replaceChild(cloneExpected(expectedChild), currentChild);
      stats.replaced += 1;
      continue;
    }
    patchNode(currentChild, expectedChild);
  }

  while (current.childNodes.length > expectedChildren.length) {
    current.lastChild.remove();
    stats.removed += 1;
  }
}

function patchNode(current, expected) {
  if (current.nodeType === Node.TEXT_NODE || current.nodeType === Node.COMMENT_NODE) {
    if (current.nodeValue !== expected.nodeValue) {
      current.nodeValue = expected.nodeValue;
      stats.text += 1;
    }
    return;
  }
  patchAttributes(current, expected);
  patchChildren(current, expected);
  patchFormState(current, expected);
}

const active = document.activeElement;
const activeKey = nodeKey(active);
const selection = active && 'selectionStart' in active ? {
  start: active.selectionStart,
  end: active.selectionEnd,
  direction: active.selectionDirection
} : null;

patchNode(currentRoot, expectedRoot);

if (activeKey && document.activeElement !== active) {
  const replacement = [...currentRoot.querySelectorAll('*')]
    .find(node => nodeKey(node) === activeKey);
  if (replacement) {
    replacement.focus({ preventScroll: true });
    if (selection && 'setSelectionRange' in replacement) {
      const length = replacement.value.length;
      replacement.setSelectionRange(
        Math.min(selection.start, length),
        Math.min(selection.end, length),
        selection.direction
      );
    }
  }
}

stats.matches = currentRoot.isEqualNode(expectedRoot);
window.__schnelluiLastDomDiff = stats;
return stats;
})()"#;

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
