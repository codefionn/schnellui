// Runs the live DOM reconciler (reconcile.js) against a minimal fake DOM.
// Usage: node reconcile_test.mjs <path to reconcile.js>
import { readFileSync } from 'node:fs';
import process from 'node:process';
import assert from 'node:assert/strict';

(0, eval)(readFileSync(process.argv[2], 'utf8'));
const reconcileRoot = globalThis.__schnelluiReconcileRoot;
assert.equal(typeof reconcileRoot, 'function');

const doc = { activeElement: null };

class FakeNode {
  constructor(nodeType, tagName = null) {
    this.nodeType = nodeType;
    this.tagName = tagName;
    this.parentNode = null;
    this._children = [];
    this.childNodesReads = 0;
    this.attrs = new Map();
    this.nodeValue = null;
    this.value = '';
    this.checked = false;
    this.indeterminate = false;
    this.type = 'text';
  }
  // Every read of `childNodes` is counted per node; the reconciler must not
  // read it once per child.
  get childNodes() { this.childNodesReads += 1; return this._children.slice(); }
  get firstChild() { return this._children[0] ?? null; }
  get nextSibling() {
    const siblings = this.parentNode?._children;
    return siblings ? siblings[siblings.indexOf(this) + 1] ?? null : null;
  }
  get id() { return this.attrs.get('id') ?? ''; }
  get attributes() { return [...this.attrs].map(([name, value]) => ({ name, value })); }
  get lastChild() { return this._children[this._children.length - 1] ?? null; }
  getAttribute(name) { return this.attrs.get(name) ?? null; }
  hasAttribute(name) { return this.attrs.has(name); }
  setAttribute(name, value) { this.attrs.set(name, String(value)); }
  removeAttribute(name) { this.attrs.delete(name); }
  contains(node) { for (let n = node; n; n = n.parentNode) if (n === this) return true; return false; }
  detach() {
    if (!this.parentNode) return;
    // Browsers blur an element when it leaves the document.
    if (doc.activeElement && this.contains(doc.activeElement)) doc.activeElement = null;
    const siblings = this.parentNode._children;
    siblings.splice(siblings.indexOf(this), 1);
    this.parentNode = null;
  }
  remove() { this.detach(); }
  insertBefore(node, reference) {
    node.detach();
    const at = reference ? this._children.indexOf(reference) : this._children.length;
    this._children.splice(at, 0, node);
    node.parentNode = this;
    return node;
  }
  appendChild(node) { return this.insertBefore(node, null); }
  replaceChild(node, old) { this.insertBefore(node, old); old.detach(); return old; }
  focus() { doc.activeElement = this; }
  isEqualNode(other) {
    if (this.nodeType !== other.nodeType || this.tagName !== other.tagName) return false;
    if (this.nodeValue !== other.nodeValue) return false;
    if (this.attrs.size !== other.attrs.size) return false;
    for (const [k, v] of this.attrs) if (other.attrs.get(k) !== v) return false;
    if (this._children.length !== other._children.length) return false;
    return this._children.every((c, i) => c.isEqualNode(other._children[i]));
  }
}

function el(tag, attrs = {}, children = []) {
  const node = new FakeNode(1, tag.toUpperCase());
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
  for (const child of children) node.appendChild(child);
  return node;
}
function text(value) {
  const node = new FakeNode(3);
  node.nodeValue = value;
  return node;
}
function importNode(node) {
  const copy = new FakeNode(node.nodeType, node.tagName);
  copy.nodeValue = node.nodeValue;
  for (const [k, v] of node.attrs) copy.setAttribute(k, v);
  copy.value = node.value;
  for (const child of node.childNodes) copy.appendChild(importNode(child));
  return copy;
}
const patch = (current, expected) =>
  reconcileRoot(current, expected, importNode, () => doc.activeElement);
const item = (key, label = key) => el('li', { 'data-sui-key': key }, [text(label)]);
const order = root => root.childNodes.map(c => c.getAttribute('data-sui-key'));

const tests = {
  'keyed reorder of 1000 children keeps every node'() {
    const keys = Array.from({ length: 1000 }, (_, i) => `k${i}`);
    const current = el('ul', {}, keys.map(k => item(k)));
    const before = new Map(current.childNodes.map(n => [n.getAttribute('data-sui-key'), n]));
    const shuffled = [...keys].reverse();
    for (let i = 0; i < shuffled.length; i += 2) {
      const j = (i * 7919) % shuffled.length;
      [shuffled[i], shuffled[j]] = [shuffled[j], shuffled[i]];
    }
    const expected = el('ul', {}, shuffled.map(k => item(k)));
    const stats = patch(current, expected);
    assert.deepEqual(order(current), shuffled);
    assert.equal(stats.matches, true);
    assert.equal(stats.inserted, 0);
    assert.equal(stats.removed, 0);
    for (const child of current.childNodes) {
      assert.equal(child, before.get(child.getAttribute('data-sui-key')));
    }
  },
  'patching does not index the live child list per child'() {
    const keys = Array.from({ length: 500 }, (_, i) => `k${i}`);
    const current = el('ul', {}, keys.map(k => item(k)));
    const expected = el('ul', {}, [...keys].reverse().map(k => item(k)));
    const stats = patch(current, expected);
    assert.equal(stats.matches, true);
    // One snapshot to walk the list, not one lookup per child.
    assert.ok(current.childNodesReads <= 2, `childNodes read ${current.childNodesReads} times`);
  },
  'surplus tail is removed after a cursor walk'() {
    const current = el('ul', {}, ['a', 'b', 'c', 'd', 'e'].map(k => item(k)));
    const [a, , c] = current.childNodes;
    const expected = el('ul', {}, ['a', 'c'].map(k => item(k)));
    const stats = patch(current, expected);
    assert.equal(stats.matches, true);
    assert.equal(stats.removed, 3);
    assert.deepEqual([...current.childNodes], [a, c]);
  },
  'insert in the middle keeps the neighbours'() {
    const current = el('ul', {}, ['a', 'b', 'c', 'd'].map(k => item(k)));
    const [a, b, c, d] = current.childNodes;
    const expected = el('ul', {}, ['a', 'b', 'new', 'c', 'd'].map(k => item(k)));
    const stats = patch(current, expected);
    assert.deepEqual(order(current), ['a', 'b', 'new', 'c', 'd']);
    assert.equal(stats.matches, true);
    assert.equal(stats.inserted, 1);
    assert.equal(stats.moved, 0);
    assert.deepEqual([current.childNodes[0], current.childNodes[1], current.childNodes[3], current.childNodes[4]], [a, b, c, d]);
  },
  'removal drops the right node'() {
    const current = el('ul', {}, ['a', 'b', 'c', 'd'].map(k => item(k)));
    const [a, , c, d] = current.childNodes;
    const expected = el('ul', {}, ['a', 'c', 'd'].map(k => item(k)));
    const stats = patch(current, expected);
    assert.deepEqual(order(current), ['a', 'c', 'd']);
    assert.equal(stats.matches, true);
    assert.deepEqual([...current.childNodes], [a, c, d]);
  },
  'unkeyed text slot before keyed children is patched in place'() {
    const current = el('div', {}, [text('old'), item('a'), item('b')]);
    const expected = el('div', {}, [text('new'), item('b'), item('a')]);
    const stats = patch(current, expected);
    assert.equal(stats.matches, true);
    assert.equal(current.childNodes[0].nodeValue, 'new');
    assert.deepEqual(order(el('x', {}, current.childNodes.slice(1))), ['b', 'a']);
  },
  'focus survives a keyed reorder'() {
    const input = el('input', { 'data-sui-key': 'field' });
    const current = el('div', {}, [item('a'), input, item('b')]);
    input.focus();
    const expected = el('div', {}, [item('b'), el('input', { 'data-sui-key': 'field' }), item('a')]);
    const stats = patch(current, expected);
    assert.equal(stats.matches, true);
    assert.equal(doc.activeElement, input);
    assert.equal(current.childNodes[1], input);
  },
  'focus moves to the replacement when the keyed input is recreated'() {
    const input = el('input', { 'data-sui-key': 'field' });
    const current = el('div', {}, [el('section', {}, [input])]);
    input.focus();
    // The wrapper's tag changed, so the whole subtree is cloned.
    const expected = el('div', {}, [el('article', {}, [el('input', { 'data-sui-key': 'field' })])]);
    const stats = patch(current, expected);
    assert.equal(stats.matches, true);
    assert.notEqual(doc.activeElement, input);
    assert.equal(doc.activeElement, current.childNodes[0].childNodes[0]);
  },
  'focus restore picks the node at the same key path, not the first with the key'() {
    // Two elements share a semantic key; the first one holds focus.
    const field = () => el('input', { 'data-sui-key': 'field' });
    const focused = field();
    const current = el('div', {}, [
      el('section', { 'data-sui-key': 'one' }, [focused]),
      el('section', { 'data-sui-key': 'two' }, [field()]),
    ]);
    focused.focus();
    // Both sections are recreated (new tag), so the browser drops focus.
    const expected = el('div', {}, [
      el('article', { 'data-sui-key': 'one' }, [field()]),
      el('article', { 'data-sui-key': 'two' }, [field()]),
    ]);
    const stats = patch(current, expected);
    assert.equal(stats.matches, true);
    assert.equal(doc.activeElement, current.childNodes[0].childNodes[0]);
  },
};

let failed = 0;
for (const [name, run] of Object.entries(tests)) {
  doc.activeElement = null;
  try {
    run();
    console.log(`ok ${name}`);
  } catch (error) {
    failed += 1;
    console.log(`FAIL ${name}\n${error.stack}`);
  }
}
process.exit(failed ? 1 : 0);
