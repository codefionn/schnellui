(() => {
// DOM reconciler for the live HTML backend. It is installed once per page and
// called with only the new root markup, so the cost of an update does not
// include compiling a script the size of the document.
//
// The algorithm only needs a small DOM surface (childNodes, insertBefore,
// appendChild, replaceChild, remove, firstChild, nextSibling, attributes,
// parentNode) so it can be
// tested against a minimal fake DOM under node or deno.

const ELEMENT_NODE = 1;
const TEXT_NODE = 3;
const COMMENT_NODE = 8;

function nodeKey(node) {
  if (!node || node.nodeType !== ELEMENT_NODE) return null;
  if (node.id) return `id:${node.id}`;
  const explicit = node.getAttribute('data-sui-key');
  if (explicit) return `key:${explicit}`;
  const role = node.getAttribute('data-sui-role');
  const name = node.getAttribute('data-sui-name');
  return role && name ? `semantic:${role}:${name}` : null;
}

function compatible(current, expected) {
  return current.nodeType === expected.nodeType &&
    (current.nodeType !== ELEMENT_NODE || current.tagName === expected.tagName);
}

function patchAttributes(current, expected, stats) {
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
  switch (current.tagName) {
    case 'INPUT':
      if (current.type !== 'file' && current.value !== expected.value) {
        current.value = expected.value;
      }
      current.checked = expected.checked;
      current.indeterminate = expected.indeterminate;
      break;
    case 'TEXTAREA':
      if (current.value !== expected.value) current.value = expected.value;
      break;
    case 'SELECT':
      current.selectedIndex = expected.selectedIndex;
      break;
    case 'OPTION':
      current.selected = expected.selected;
      break;
    case 'DETAILS':
      current.open = expected.open;
      break;
  }
}

// `importNode(node)` copies an expected node into the live document.
function createReconciler(importNode) {
  const stats = {
    matches: false,
    attributes: 0,
    text: 0,
    inserted: 0,
    removed: 0,
    replaced: 0,
    moved: 0
  };
  // Every live node seen per key, and the subtrees cloned in from the expected
  // tree. Together they let focus restore find the active element without
  // scanning the whole document.
  const seen = new Map();
  const cloned = [];

  function clone(node) {
    const copy = importNode(node);
    cloned.push(copy);
    return copy;
  }

  function patchChildren(current, expected) {
    const expectedChildren = [...expected.childNodes];
    const currentChildren = [...current.childNodes];

    // One pass over each side builds the lookups; each keyed child is then
    // found in O(1) instead of scanning the remaining siblings.
    const available = new Map();
    for (const child of currentChildren) {
      const key = nodeKey(child);
      if (!key) continue;
      const list = available.get(key);
      if (list) list.push(child); else available.set(key, [child]);
      const everywhere = seen.get(key);
      if (everywhere) everywhere.push(child); else seen.set(key, [child]);
    }
    const lastExpected = new Map();
    const expectedKeys = expectedChildren.map(nodeKey);
    expectedKeys.forEach((key, index) => { if (key) lastExpected.set(key, index); });
    const used = new Set();

    // `cursor` is the live node at the position being filled. Walking it with
    // nextSibling avoids indexing the live NodeList, which is O(n) per lookup in
    // a browser once nodes have been moved.
    let cursor = current.firstChild;
    for (let index = 0; index < expectedChildren.length; index += 1) {
      const expectedChild = expectedChildren[index];
      let currentChild = cursor;
      // Where the cursor goes once this position is done: past the node that
      // sat here, unless a moved-in node took its place before it.
      let next = cursor ? cursor.nextSibling : null;
      const expectedKey = expectedKeys[index];

      if (expectedKey && nodeKey(currentChild) !== expectedKey) {
        const list = available.get(expectedKey);
        let match = null;
        while (list && list.length) {
          const candidate = list.shift();
          if (!used.has(candidate) && candidate.parentNode === current) {
            match = candidate;
            break;
          }
        }
        if (match) {
          current.insertBefore(match, currentChild);
          currentChild = match;
          next = cursor;
          stats.moved += 1;
        } else {
          current.insertBefore(clone(expectedChild), currentChild);
          stats.inserted += 1;
          continue;
        }
      }
      if (currentChild) used.add(currentChild);

      const currentKey = nodeKey(currentChild);
      if (!expectedKey && currentKey && (lastExpected.get(currentKey) ?? -1) > index) {
        // The keyed node in this slot is needed later; leave it for its turn.
        used.delete(currentChild);
        current.insertBefore(clone(expectedChild), currentChild);
        stats.inserted += 1;
        continue;
      }

      if (!currentChild) {
        current.appendChild(clone(expectedChild));
        stats.inserted += 1;
        continue;
      }

      cursor = next;
      if (!compatible(currentChild, expectedChild)) {
        current.replaceChild(clone(expectedChild), currentChild);
        stats.replaced += 1;
        continue;
      }
      patchNode(currentChild, expectedChild);
    }

    // Whatever the cursor has not passed is surplus.
    while (cursor) {
      const next = cursor.nextSibling;
      cursor.remove();
      stats.removed += 1;
      cursor = next;
    }
  }

  function patchNode(current, expected) {
    if (current.nodeType === TEXT_NODE || current.nodeType === COMMENT_NODE) {
      if (current.nodeValue !== expected.nodeValue) {
        current.nodeValue = expected.nodeValue;
        stats.text += 1;
      }
      return;
    }
    patchAttributes(current, expected, stats);
    patchChildren(current, expected);
    patchFormState(current, expected);
  }

  function connected(node, root) {
    for (let at = node; at; at = at.parentNode) if (at === root) return true;
    return false;
  }

  // The keys of `node`'s keyed ancestors and of `node` itself, outermost first.
  function keyPath(node, root) {
    const path = [];
    for (let at = node; at && at !== root; at = at.parentNode) {
      const key = nodeKey(at);
      if (key) path.unshift(key);
    }
    return path;
  }

  // Finds the live node with `key` at `path`: among nodes that were patched in
  // place, then inside subtrees that were freshly cloned. Keys are not unique
  // across a page (a semantic role and name can repeat), so the first node with
  // the key is not necessarily the focused one. When nothing sits at the exact
  // path, a key with a single live holder is still unambiguous.
  function find(key, root, path) {
    const found = new Set((seen.get(key) ?? []).filter(node => connected(node, root)));
    const stack = cloned.filter(node => connected(node, root));
    while (stack.length) {
      const node = stack.pop();
      if (nodeKey(node) === key) found.add(node);
      if (node.childNodes) stack.push(...node.childNodes);
    }
    const same = candidate => {
      const other = keyPath(candidate, root);
      return other.length === path.length && other.every((k, i) => k === path[i]);
    };
    for (const node of found) if (same(node)) return node;
    return found.size === 1 ? [...found][0] : null;
  }

  return { stats, patchNode, find, keyPath };
}

// Patches `currentRoot` to match `expectedRoot`, keeping focus and selection
// on the element with the same key if the browser dropped it while moving.
function reconcileRoot(currentRoot, expectedRoot, importNode, activeElement) {
  const reconciler = createReconciler(importNode);
  const active = activeElement();
  const activeKey = nodeKey(active);
  const activePath = activeKey ? reconciler.keyPath(active, currentRoot) : null;
  const selection = active && 'selectionStart' in active ? {
    start: active.selectionStart,
    end: active.selectionEnd,
    direction: active.selectionDirection
  } : null;

  reconciler.patchNode(currentRoot, expectedRoot);

  if (activeKey && activeElement() !== active) {
    const replacement = reconciler.find(activeKey, currentRoot, activePath);
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

  reconciler.stats.matches = currentRoot.isEqualNode(expectedRoot);
  return reconciler.stats;
}

if (typeof document === 'undefined') {
  // Test environments provide their own DOM.
  globalThis.__schnelluiReconcileRoot = reconcileRoot;
  return;
}
if (globalThis.__schnelluiPatch) return;

// Applies one update. `bodyHtml` is the markup inside #schnellui-root;
// `styleText` is the full stylesheet, or null when it has not changed.
globalThis.__schnelluiPatch = (bodyHtml, styleText) => {
  if (styleText !== null && styleText !== undefined) {
    // The live stylesheet is an element this script owns, so a host page's own
    // <style> is never overwritten.
    let style = document.getElementById('schnellui-native-styles');
    if (!style) {
      style = document.createElement('style');
      style.id = 'schnellui-native-styles';
      document.head.appendChild(style);
    }
    if (style.textContent !== styleText) style.textContent = styleText;
  }
  let currentRoot = document.getElementById('schnellui-root');
  if (!currentRoot) {
    currentRoot = document.createElement('main');
    currentRoot.id = 'schnellui-root';
    document.body.appendChild(currentRoot);
  }
  const expectedDocument = new DOMParser().parseFromString(
    '<!doctype html><main id="schnellui-root">' + bodyHtml + '</main>',
    'text/html'
  );
  const expectedRoot = expectedDocument.getElementById('schnellui-root');
  const stats = reconcileRoot(
    currentRoot,
    expectedRoot,
    node => document.importNode(node, true),
    () => document.activeElement
  );
  window.__schnelluiLastDomDiff = stats;
  return stats;
};
})()
