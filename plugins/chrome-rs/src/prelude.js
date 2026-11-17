// Minimal browser environment for QuickJS. The page DOM arrives as JSON (__dom_json), scripts mutate this JS-side
// tree, and __serialize() hands the result back to Rust. Natives: __parse(html) -> JSON array of nodes, __url.
(function (g) {
'use strict';
const VOID = new Set(['area','base','br','col','embed','hr','img','input','link','meta','source','track','wbr','param']);
const camel = s => s.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
const kebab = s => s.replace(/[A-Z]/g, c => '-' + c.toLowerCase());
const esc = s => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
// registry of custom elements (see the custom element section below); it must exist before the first DOM node is built
const CE = { defs: new Map(), tags: new Map(), up: null, waiters: new Map() };
const escA = s => esc(s).replace(/"/g, '&quot;');

class Event {
  constructor(type, init) { this.type = type; this.bubbles = !!(init && init.bubbles); this.cancelable = !!(init && init.cancelable); this.defaultPrevented = false; this.target = null; this.detail = init && init.detail; this.timeStamp = 0; }
  preventDefault() { this.defaultPrevented = true; }
  stopPropagation() { this._stop = true; }
  stopImmediatePropagation() { this._stop = true; this._stopNow = true; }
}
class CustomEvent extends Event {}

class EventTarget {
  // options: `true` / `{ capture, once }`; the same function twice with the same capture flag is added once
  addEventListener(t, f, o) {
    if (!f) return;
    const cap = o === true || !!(o && o.capture), L = this._l = this._l || {};
    const a = L[t] = L[t] || [];
    if (a.some(x => x.f === f && x.cap === cap)) return;
    a.push({ f, cap, once: !!(o && o.once) });
  }
  removeEventListener(t, f, o) {
    const cap = o === true || !!(o && o.capture);
    if (this._l && this._l[t]) this._l[t] = this._l[t].filter(x => !(x.f === f && x.cap === cap));
  }
  dispatchEvent(e) {
    e.target = e.target || this;
    const path = [];
    for (let n = this; n; n = n.parentNode || (n === document ? g : n.nodeType === 11 ? n.host : null)) path.push(n);
    e.composedPath = () => path.slice();
    const run = (n, phase) => {
      try { e.currentTarget = n; } catch (x) {} // Svelte redefines it as a getter-only property
      e.eventPhase = phase;
      if (phase !== 1) {
        // `el.onclick = fn` and inline `onclick="..."` attributes; returning false cancels the default action
        let oh = n['on' + e.type];
        if (typeof oh !== 'function' && n._a && typeof n._a['on' + e.type] === 'string') {
          n._oh = n._oh || {};
          if (!(e.type in n._oh)) { try { n._oh[e.type] = new Function('event', n._a['on' + e.type]); } catch (x) { n._oh[e.type] = null; } }
          oh = n._oh[e.type];
        }
        if (typeof oh === 'function') { try { if (oh.call(n, e) === false) e.preventDefault(); } catch (x) {} }
      }
      const ls = n._l && n._l[e.type];
      if (ls) for (const l of ls.slice()) {
        if (phase === 1 ? !l.cap : phase === 3 && l.cap) continue;
        if (l.once) n._l[e.type] = n._l[e.type].filter(x => x !== l);
        try { typeof l.f === 'function' ? l.f.call(n, e) : l.f.handleEvent(e); } catch (x) { g.__log('listener ' + e.type + ': ' + (x && (x.message + ' ' + x.stack) || x)); }
        if (e._stopNow) return;
      }
    };
    // capture from the outermost ancestor down, then the target, then (if the event bubbles) back up
    for (let i = path.length - 1; i > 0 && !e._stop; i--) run(path[i], 1);
    if (!e._stop) run(path[0], 2);
    for (let i = 1; i < path.length && e.bubbles && !e._stop; i++) run(path[i], 3);
    e.eventPhase = 0; try { e.currentTarget = null; } catch (x) {}
    return !e.defaultPrevented;
  }
}

function descendants(root) {
  const out = [], st = root.childNodes.slice().reverse();
  while (st.length) { const n = st.pop(); if (n.nodeType === 1) { out.push(n); for (let i = n.childNodes.length - 1; i >= 0; i--) st.push(n.childNodes[i]); } }
  return out;
}

class Node extends EventTarget {
  constructor() { super(); this.childNodes = []; this.parentNode = null; }
  get firstChild() { return this.childNodes[0] || null; }
  get lastChild() { return this.childNodes[this.childNodes.length - 1] || null; }
  get nextSibling() { const p = this.parentNode; return p ? p.childNodes[p.childNodes.indexOf(this) + 1] || null : null; }
  get previousSibling() { const p = this.parentNode; return p ? p.childNodes[p.childNodes.indexOf(this) - 1] || null : null; }
  get parentElement() { const p = this.parentNode; return p && p.nodeType === 1 ? p : null; }
  get children() { return this.childNodes.filter(n => n.nodeType === 1); }
  get childElementCount() { return this.children.length; }
  get firstElementChild() { return this.children[0] || null; }
  get lastElementChild() { const c = this.children; return c[c.length - 1] || null; }
  get nextElementSibling() { for (let n = this.nextSibling; n; n = n.nextSibling) if (n.nodeType === 1) return n; return null; }
  get previousElementSibling() { for (let n = this.previousSibling; n; n = n.previousSibling) if (n.nodeType === 1) return n; return null; }
  get ownerDocument() { return document; }
  get isConnected() { for (let n = this; n; n = n.parentNode || (n.nodeType === 11 ? n.host : null)) if (n === document) return true; return false; }
  hasChildNodes() { return this.childNodes.length > 0; }
  appendChild(n) { return this.insertBefore(n, null); }
  insertBefore(n, ref) {
    if (n.nodeType === 11) { for (const c of n.childNodes.slice()) this.insertBefore(c, ref); return n; }
    if (n.parentNode) n.parentNode.removeChild(n);
    n.parentNode = this;
    const i = ref ? this.childNodes.indexOf(ref) : -1;
    if (i < 0) this.childNodes.push(n); else this.childNodes.splice(i, 0, n);
    if (CE.defs.size) ceConnect(n);
    return n;
  }
  removeChild(n) { const i = this.childNodes.indexOf(n); if (i >= 0) { this.childNodes.splice(i, 1); n.parentNode = null; if (CE.defs.size) ceDisconnect(n); } return n; }
  replaceChild(n, old) { this.insertBefore(n, old); this.removeChild(old); return old; }
  remove() { if (this.parentNode) this.parentNode.removeChild(this); }
  append(...a) { for (const x of a) this.appendChild(typeof x === 'object' ? x : document.createTextNode(String(x))); }
  prepend(...a) { const f = this.firstChild; for (const x of a) this.insertBefore(typeof x === 'object' ? x : document.createTextNode(String(x)), f); }
  before(...a) { const p = this.parentNode; if (p) for (const x of a) p.insertBefore(typeof x === 'object' ? x : document.createTextNode(String(x)), this); }
  after(...a) { const p = this.parentNode, nx = this.nextSibling; if (p) for (const x of a) p.insertBefore(typeof x === 'object' ? x : document.createTextNode(String(x)), nx); }
  contains(n) { for (; n; n = n.parentNode) if (n === this) return true; return false; }
  get textContent() { return this.childNodes.map(c => c.nodeType === 8 ? '' : c.textContent).join(''); }
  set textContent(v) {
    this.childNodes.forEach(c => c.parentNode = null); this.childNodes = [];
    if (v !== '' && v != null) this.appendChild(document.createTextNode(String(v)));
  }
  get innerText() { return this.textContent; }
  set innerText(v) { this.textContent = v; }
  cloneNode(deep, inert) {
    let c;
    if (this.nodeType === 3) c = new Text(this.data);
    else if (this.nodeType === 8) c = new Comment(this.data);
    else if (this.nodeType === 11) c = new DocumentFragment();
    else { const C = !inert && CE.defs.get(this.localName); c = C ? new C() : new Element(this.localName); Object.assign(c._a, this._a); } // a defined custom element is constructed (upgraded) at once, as importNode does; template content stays inert
    if (deep) for (const k of this.childNodes) c.appendChild(k.cloneNode(true, inert));
    return c;
  }
}

class Text extends Node {
  constructor(d) { super(); this.data = String(d); this.nodeType = 3; this.nodeName = '#text'; }
  get textContent() { return this.data; }
  set textContent(v) { this.data = String(v); }
  get nodeValue() { return this.data; }
  set nodeValue(v) { this.data = String(v); }
}
class Comment extends Text { constructor(d) { super(d); this.nodeType = 8; this.nodeName = '#comment'; } }
class DocumentFragment extends Node {
  constructor() { super(); this.nodeType = 11; this.nodeName = '#document-fragment'; }
  querySelector(s) { return qsa(this, s, true)[0] || null; }
  querySelectorAll(s) { return qsa(this, s, false); }
}

function ser(n) {
  if (n.nodeType === 3) return esc(n.data);
  if (n.nodeType === 8) return '<!--' + n.data + '-->';
  if (n.nodeType !== 1) return '';
  const a = Object.keys(n._a).map(k => ' ' + k + '="' + escA(n._a[k]) + '"').join('');
  if (VOID.has(n.localName)) return '<' + n.localName + a + '>';
  const raw = n.localName === 'script' || n.localName === 'style';
  return '<' + n.localName + a + '>' + n.childNodes.map(c => raw && c.nodeType === 3 ? c.data : ser(c)).join('') + '</' + n.localName + '>';
}
function fromJSON(arr) {
  return arr.map(o => {
    if (typeof o === 'string') return new Text(o);
    if (o.m !== undefined) return new Comment(o.m);
    const e = new Element(o.t); e._a = o.a || {};
    for (const c of fromJSON(o.c || [])) { c.parentNode = e; e.childNodes.push(c); }
    if (CE.defs.size && CE.defs.has(e.localName)) ceUpgrade(e, CE.defs.get(e.localName));
    return e;
  });
}

const selCache = new Map();
function parseCompound(t) {
  const c = { tag: null, id: null, cls: [], attrs: [], first: false, last: false, nots: [], bad: false };
  const re = /([#.]?[\w-]+|\*|\[[^\]]+\]|::?[\w-]+(?:\([^)]*\))?)/g;
  let m;
  while ((m = re.exec(t))) {
    const x = m[1];
    if (x[0] === '#') c.id = x.slice(1);
    else if (x[0] === '.') c.cls.push(x.slice(1));
    else if (x[0] === '[') {
      const a = /\[\s*([\w-]+)\s*(?:([~|^$*]?=)\s*["']?([^"'\]]*)["']?)?\s*\]/.exec(x);
      if (a) c.attrs.push([a[1].toLowerCase(), a[2], a[3]]); else c.bad = true;
    } else if (x[0] === ':') {
      if (x === ':first-child') c.first = true; else if (x === ':last-child') c.last = true;
      else if (x.startsWith(':not(')) c.nots.push(x.slice(5, -1));
      else c.bad = true;
    } else if (x !== '*') c.tag = x.toLowerCase();
  }
  return c;
}
function parseSel(s) {
  let r = selCache.get(s);
  if (r) return r;
  r = s.split(',').map(p => {
    const toks = p.trim().replace(/\s*([>+~])\s*/g, ' $1 ').split(/\s+/).filter(Boolean);
    const parts = []; let comb = ' ';
    for (const t of toks) { if (t.length === 1 && '>+~'.includes(t)) { comb = t; continue; } parts.push({ comb, c: parseCompound(t) }); comb = ' '; }
    return parts;
  }).filter(p => p.length);
  selCache.set(s, r);
  return r;
}
function matchC(el, c) {
  if (c.bad) return false;
  if (c.tag && el.localName !== c.tag) return false;
  if (c.id && el._a.id !== c.id) return false;
  if (c.cls.length) { const cl = (el._a['class'] || '').split(/\s+/); for (const k of c.cls) if (!cl.includes(k)) return false; }
  for (const [n, op, v] of c.attrs) {
    const x = el._a[n]; if (x === undefined) return false;
    if (op === '=' && x !== v) return false;
    if (op === '^=' && !x.startsWith(v)) return false;
    if (op === '$=' && !x.endsWith(v)) return false;
    if (op === '*=' && !x.includes(v)) return false;
    if (op === '~=' && !x.split(/\s+/).includes(v)) return false;
  }
  if (c.first && el.previousElementSibling) return false;
  if (c.last && el.nextElementSibling) return false;
  for (const n of c.nots) if (matchesSel(el, n)) return false;
  return true;
}
function matchParts(el, parts, i) {
  if (!matchC(el, parts[i].c)) return false;
  if (i === 0) return true;
  switch (parts[i].comb) {
    case ' ': for (let p = el.parentElement; p; p = p.parentElement) if (matchParts(p, parts, i - 1)) return true; return false;
    case '>': return !!el.parentElement && matchParts(el.parentElement, parts, i - 1);
    case '+': return !!el.previousElementSibling && matchParts(el.previousElementSibling, parts, i - 1);
    default: for (let p = el.previousElementSibling; p; p = p.previousElementSibling) if (matchParts(p, parts, i - 1)) return true; return false;
  }
}
function matchesSel(el, s) { return parseSel(s).some(p => matchParts(el, p, p.length - 1)); }
function qsa(root, s, first) {
  const out = [];
  for (const e of descendants(root)) if (matchesSel(e, s)) { out.push(e); if (first) break; }
  return out;
}

function makeStyle(el) {
  const o = {};
  (el._a.style || '').split(';').forEach(d => { const i = d.indexOf(':'); if (i > 0) o[camel(d.slice(0, i).trim())] = d.slice(i + 1).trim(); });
  const sync = () => { el._a.style = Object.keys(o).filter(k => o[k] !== '').map(k => kebab(k) + ':' + o[k]).join(';'); };
  return new Proxy(o, {
    set(t, k, v) { t[k] = String(v); sync(); return true; },
    get(t, k) {
      if (typeof k === 'symbol') return undefined;
      if (k === 'cssText') return el._a.style || '';
      if (k === 'setProperty') return (p, v) => { t[camel(p)] = String(v); sync(); };
      if (k === 'removeProperty') return p => { delete t[camel(p)]; sync(); };
      if (k === 'getPropertyValue') return p => t[camel(p)] || '';
      return k in t ? t[k] : '';
    },
  });
}

// Plain elements sit one prototype below Element.prototype (like HTMLElement.prototype in browsers): Svelte collects property setters (`hidden`, `value`...) from that part of the chain only
const HTMLProto = Object.create(null);
class Element extends Node {
  constructor(tag) { super(); if (new.target === Element) Object.setPrototypeOf(this, HTMLProto); tag = String(tag); this.nodeType = 1; this.localName = tag.toLowerCase(); this.tagName = tag.toUpperCase(); this.nodeName = this.tagName; this._a = {}; }
  getAttribute(n) { n = n.toLowerCase(); return n in this._a ? this._a[n] : null; }
  setAttribute(n, v) { n = n.toLowerCase(); const o = n in this._a ? this._a[n] : null; this._a[n] = String(v); if (n === 'style') this._style = null; if (this._ce) ceAttr(this, n, o, String(v)); }
  removeAttribute(n) { n = n.toLowerCase(); const o = n in this._a ? this._a[n] : null; delete this._a[n]; if (n === 'style') this._style = null; if (this._ce && o !== null) ceAttr(this, n, o, null); }
  hasAttribute(n) { return n.toLowerCase() in this._a; }
  toggleAttribute(n, f) { const has = this.hasAttribute(n), want = f === undefined ? !has : !!f; if (want && !has) this.setAttribute(n, ''); else if (!want && has) this.removeAttribute(n); return want; }
  get id() { return this._a.id || ''; } set id(v) { this._a.id = String(v); }
  get className() { return this._a['class'] || ''; } set className(v) { this._a['class'] = String(v); }
  get hidden() { return 'hidden' in this._a; } set hidden(v) { if (v) this._a.hidden = ''; else delete this._a.hidden; }
  get classList() {
    const el = this, get = () => (el._a['class'] || '').split(/\s+/).filter(Boolean), set = a => { el._a['class'] = a.join(' '); };
    return {
      add(...c) { const a = get(); for (const x of c) if (!a.includes(x)) a.push(x); set(a); },
      remove(...c) { set(get().filter(x => !c.includes(x))); },
      toggle(c, f) { const has = get().includes(c), want = f === undefined ? !has : !!f; if (want && !has) this.add(c); else if (!want && has) this.remove(c); return want; },
      contains(c) { return get().includes(c); },
      replace(a, b) { set(get().map(x => x === a ? b : x)); return true; },
      get length() { return get().length; }, item(i) { return get()[i] || null; }, toString() { return get().join(' '); },
    };
  }
  // an array that also answers `el.attributes.id` and `getNamedItem` (jQuery 1.x reads `attributes[name].expando`)
  get attributes() {
    const l = Object.keys(this._a).map(k => ({ name: k, nodeName: k, localName: k, value: this._a[k], nodeValue: this._a[k], specified: true }));
    for (const a of l) if (!(a.name in l)) l[a.name] = a;
    l.getNamedItem = n => l[n] || null; l.item = i => l[i] || null;
    return l;
  }
  get dataset() {
    const a = this._a;
    return new Proxy({}, { get: (t, k) => typeof k === 'symbol' ? undefined : a['data-' + kebab(k)], set: (t, k, v) => { a['data-' + kebab(k)] = String(v); return true; } });
  }
  get style() { return this._style || (this._style = makeStyle(this)); }
  set style(v) { this._a.style = String(v); this._style = null; }
  get innerHTML() { const raw = this.localName === 'script' || this.localName === 'style'; return this.childNodes.map(c => raw && c.nodeType === 3 ? c.data : ser(c)).join(''); }
  set innerHTML(h) {
    this.childNodes.forEach(c => c.parentNode = null); this.childNodes = [];
    for (const c of fromJSON(JSON.parse(__parse(String(h))))) this.appendChild(c);
  }
  get outerHTML() { return ser(this); }
  insertAdjacentHTML(pos, h) {
    const nodes = fromJSON(JSON.parse(__parse(String(h))));
    if (pos === 'beforeend') nodes.forEach(n => this.appendChild(n));
    else if (pos === 'afterbegin') { const f = this.firstChild; nodes.forEach(n => this.insertBefore(n, f)); }
    else if (pos === 'beforebegin' && this.parentNode) nodes.forEach(n => this.parentNode.insertBefore(n, this));
    else if (pos === 'afterend' && this.parentNode) { const nx = this.nextSibling; nodes.forEach(n => this.parentNode.insertBefore(n, nx)); }
  }
  querySelector(s) { return qsa(this, s, true)[0] || null; }
  querySelectorAll(s) { return qsa(this, s, false); }
  getElementsByTagName(t) { t = t.toLowerCase(); return descendants(this).filter(e => t === '*' || e.localName === t); }
  getElementsByClassName(c) { const cs = c.split(/\s+/).filter(Boolean); return descendants(this).filter(e => cs.every(x => (e._a['class'] || '').split(/\s+/).includes(x))); }
  matches(s) { return matchesSel(this, s); }
  closest(s) { for (let n = this; n && n.nodeType === 1; n = n.parentNode) if (matchesSel(n, s)) return n; return null; }
  click() { this.dispatchEvent(new Event('click', { bubbles: true, cancelable: true })); }
  focus() {} blur() {} scrollIntoView() {} animate() { return { cancel() {}, finished: Promise.resolve() }; }
  getBoundingClientRect() { const b = this._box || [0, 0, 0, 0]; return { x: b[0], y: b[1], left: b[0], top: b[1], width: b[2], height: b[3], right: b[0] + b[2], bottom: b[1] + b[3] }; }
  getClientRects() { return this._box ? [this.getBoundingClientRect()] : []; }
  get offsetWidth() { return Math.round((this._box || [0, 0, 0, 0])[2]); } get offsetHeight() { return Math.round((this._box || [0, 0, 0, 0])[3]); }
  get clientWidth() { return this.offsetWidth; } get clientHeight() { return this.offsetHeight; }
  get offsetLeft() { return Math.round((this._box || [0, 0, 0, 0])[0]); } get offsetTop() { return Math.round((this._box || [0, 0, 0, 0])[1]); }
  get scrollTop() { return this === document.documentElement || this.localName === 'body' ? g.scrollY : 0; } set scrollTop(v) {} get scrollHeight() { return this.offsetHeight; } get scrollWidth() { return this.offsetWidth; }
}
Object.setPrototypeOf(HTMLProto, Element.prototype);
for (const p of ['href', 'src', 'type', 'name', 'title', 'alt', 'rel', 'placeholder', 'target', 'action', 'method', 'lang', 'role'])
  Object.defineProperty(Element.prototype, p, { get() { return this._a[p] || ''; }, set(v) { this._a[p] = String(v); }, configurable: true });
// `a.href` is the absolute address (the raw text stays in getAttribute('href'))
Object.defineProperty(Element.prototype, 'href', { get() { const v = this._a.href; if (v === undefined) return ''; if (!isLink(this)) return v; if (v[0] === '#') return loc._h.replace(/#.*$/, '') + v; try { return __resolve(loc._h, v); } catch (e) { return v; } }, set(v) { this._a.href = String(v); }, configurable: true });
// `src` of script, img, iframe and the media tags is absolute too (webpack finds its public path from script.src)
Object.defineProperty(Element.prototype, 'src', { get() { const v = this._a.src; if (v === undefined) return ''; if (!/^(script|img|iframe|source|audio|video|embed|track|input)$/.test(this.localName)) return v; if (v[0] === '#') return loc._h.replace(/#.*$/, '') + v; try { return __resolve(loc._h, v); } catch (e) { return v; } }, set(v) { this._a.src = String(v); }, configurable: true });
const optVal = o => o._a.value !== undefined ? o._a.value : o.textContent.trim();
Object.defineProperty(Element.prototype, 'value', {
  get() {
    if (this.localName === 'textarea') return this.textContent;
    if (this.localName === 'select') { const o = descendants(this).filter(x => x.localName === 'option'); const s = o.find(x => 'selected' in x._a) || o[0]; return s ? optVal(s) : ''; }
    return this._a.value || '';
  },
  set(v) {
    v = String(v);
    if (this.localName === 'textarea') this.textContent = v;
    else if (this.localName === 'select') { for (const o of descendants(this).filter(x => x.localName === 'option')) { if (optVal(o) === v) o._a.selected = ''; else delete o._a.selected; } }
    else this._a.value = v;
  }, configurable: true });
for (const p of ['checked', 'disabled', 'selected', 'required'])
  Object.defineProperty(Element.prototype, p, { get() { return p in this._a; }, set(v) { if (v) this._a[p] = ''; else delete this._a[p]; }, configurable: true });

class Document extends Node {
  constructor() { super(); this.nodeType = 9; this.nodeName = '#document'; this.readyState = 'loading'; this.referrer = ''; this.characterSet = 'UTF-8'; }
  get cookie() { return __cookie_get(); } set cookie(v) { __cookie_set(String(v)); }
  createElement(t) { const C = CE.defs.get(String(t).toLowerCase()); return C ? new C() : new Element(t); }
  createElementNS(ns, t) { const e = new Element(t); e._ns = ns; return e; }
  createTextNode(d) { return new Text(d); }
  createComment(d) { return new Comment(d === undefined ? '' : d); }
  createDocumentFragment() { return new DocumentFragment(); }
  createEvent() { return new Event(''); }
  getElementById(id) { return descendants(this).find(e => e._a.id === id) || null; }
  querySelector(s) { return qsa(this, s, true)[0] || null; }
  querySelectorAll(s) { return qsa(this, s, false); }
  getElementsByTagName(t) { t = t.toLowerCase(); return descendants(this).filter(e => t === '*' || e.localName === t); }
  getElementsByClassName(c) { return this.documentElement ? this.documentElement.getElementsByClassName(c) : []; }
  get documentElement() { return this.childNodes.find(c => c.nodeType === 1) || null; }
  get head() { return this.documentElement && this.documentElement.children.find(c => c.localName === 'head') || null; }
  get body() { return this.documentElement && this.documentElement.children.find(c => c.localName === 'body') || null; }
  get title() { const t = this.querySelector('title'); return t ? t.textContent.trim() : ''; }
  set title(v) { let t = this.querySelector('title'); if (!t) { t = this.createElement('title'); this.head.appendChild(t); } t.textContent = v; }
  get defaultView() { return g; } get currentScript() { const n = g.__cs; if (!n) return null; const e = this.createElement('script'); if (!n.includes('#inline-')) e.setAttribute('src', n); return e; } get location() { return loc; } set location(v) { nav(v); }
  get visibilityState() { return 'visible'; } get hidden() { return false; }
  hasFocus() { return true; }
  write(h) { const b = this.body || this.documentElement; for (const c of fromJSON(JSON.parse(__parse(String(h))))) b.appendChild(c); }
  writeln(h) { this.write(h); }
  open() {} close() {}
  get implementation() {
    const mk = t => { const d = new Document(), h = d.createElement('html'), hd = d.createElement('head'), b = d.createElement('body'); d.appendChild(h); h.appendChild(hd); h.appendChild(b); if (t) { const e = d.createElement('title'); e.textContent = String(t); hd.appendChild(e); } return d; };
    return { createHTMLDocument: mk, createDocument: () => new Document(), hasFeature: () => true };
  }
}

const document = new Document();
const dom = JSON.parse(__dom_json);
for (const n of fromJSON([dom])) document.appendChild(n);

const loc = { _h: '', assign(u) { nav(u); }, replace(u) { nav(u); }, reload() {}, toString() { return this._h; } };
// location changes navigate the tab (a bare #fragment only updates the address)
const nav = u => {
  u = String(u);
  if (u[0] === '#') { setLoc(loc._h.replace(/#.*$/, '') + u); g.dispatchEvent(new Event('hashchange')); }
  else __navigate(__resolve(loc._h, u));
};
Object.defineProperty(loc, 'href', { get() { return this._h; }, set(v) { nav(v); }, enumerable: true });
Object.defineProperty(loc, 'hash', { get() { return this._hash; }, set(v) { nav('#' + String(v).replace(/^#/, '')); }, enumerable: true });
Object.defineProperty(g, 'location', { get: () => loc, set: v => nav(v), configurable: true });
function setLoc(h) {
  const m = /^(\w+:)(?:\/\/([^\/?#]*))?([^?#]*)(\?[^#]*)?(#.*)?$/.exec(h) || [];
  Object.assign(loc, { _h: h, _hash: m[5] || '', protocol: m[1] || '', host: m[2] || '', hostname: (m[2] || '').replace(/:\d+$/, ''), port: ((m[2] || '').match(/:(\d+)$/) || [])[1] || '',
    pathname: m[3] || '/', search: m[4] || '', origin: m[2] !== undefined ? (m[1] || '') + '//' + m[2] : 'null' });
}
setLoc(__url);

const lsData = JSON.parse(__ls);
const store = (init) => { const d = init || {}; return { getItem: k => k in d ? d[k] : null, setItem: (k, v) => { d[k] = String(v); }, removeItem: k => { delete d[k]; }, clear() { for (const k in d) delete d[k]; }, key: i => Object.keys(d)[i] || null, get length() { return Object.keys(d).length; } }; };
let timers = [], tid = 0, now = 0, perfT = 0, ran = 0;
const cleared = new Set(); // ids cleared while their own callback was running must not be re-armed
const noop = () => {};
class Observer { constructor() {} observe() {} unobserve() {} disconnect() {} takeRecords() { return []; } }
// everything counts as visible, so lazy-loaded and scroll-revealed content appears
class IntersectionObs extends Observer {
  constructor(cb) { super(); this.cb = cb; this.root = null; }
  observe(el) { g.setTimeout(() => { try { const r = el.getBoundingClientRect(); this.cb([{ target: el, isIntersecting: true, intersectionRatio: 1, boundingClientRect: r, intersectionRect: r, rootBounds: null, time: now }], this); } catch (e) {} }, 0); }
}
class ResizeObs extends Observer {
  constructor(cb) { super(); this.cb = cb; }
  observe(el) { g.setTimeout(() => { try { const r = el.getBoundingClientRect(); this.cb([{ target: el, contentRect: r, borderBoxSize: [{ inlineSize: r.width, blockSize: r.height }] }], this); } catch (e) {} }, 0); }
}
// ---- custom elements: `class X extends HTMLElement`, customElements.define, upgrade of elements already in the page, lifecycle callbacks.
// shortcut: shadow trees are flattened into the host when the page is serialized (see `composed`): their <style> rules apply to the whole page, `:host` and `::slotted` do not match, events are not retargeted
class HTMLElement extends Element {
  constructor() { if (CE.up) { const e = CE.up; CE.up = null; return e; } super(CE.tags.get(new.target) || 'div'); this._ce = true; }
  static [Symbol.hasInstance](o) { return this === HTMLElement ? o instanceof Element : Function.prototype[Symbol.hasInstance].call(this, o); }
}
const ceCall = (el, fn, ...a) => { try { if (typeof el[fn] === 'function') el[fn](...a); } catch (e) { __log('custom element ' + fn + ': ' + (e && (e.message + ' ' + e.stack) || e)); } };
const ceConnect = n => { if (n.nodeType !== 1) return; if (n._ce && !n._conn && n.isConnected) { n._conn = true; ceCall(n, 'connectedCallback'); } for (const c of n.childNodes.slice()) ceConnect(c); };
const ceDisconnect = n => { if (n.nodeType !== 1) return; for (const c of n.childNodes.slice()) ceDisconnect(c); if (n._conn) { n._conn = false; ceCall(n, 'disconnectedCallback'); } };
const ceAttr = (el, n, o, v) => { const obs = el.constructor && el.constructor.observedAttributes; if (obs && obs.includes(n)) ceCall(el, 'attributeChangedCallback', n, o, v); };
const ceUpgrade = (el, C) => {
  if (el._ce) return; el._ce = true;
  Object.setPrototypeOf(el, C.prototype);
  CE.up = el; try { new C(); } catch (e) { __log('custom element constructor: ' + (e && e.stack || e)); } finally { CE.up = null; }
  for (const a of Object.keys(el._a)) ceAttr(el, a, null, el._a[a]);
  ceConnect(el);
};
const customElementsApi = {
  define(name, C) { name = String(name).toLowerCase(); if (CE.defs.has(name)) throw new Error("customElements: '" + name + "' is already defined"); CE.defs.set(name, C); CE.tags.set(C, name); const found = []; const walk = n => { for (const c of n.childNodes) if (c.nodeType === 1) { if (c.localName === name) found.push(c); walk(c); } }; walk(document); for (const el of found) ceUpgrade(el, C); (CE.waiters.get(name) || []).forEach(r => r(C)); },
  get: name => CE.defs.get(String(name).toLowerCase()),
  whenDefined(name) { name = String(name).toLowerCase(); if (CE.defs.has(name)) return Promise.resolve(CE.defs.get(name)); return new Promise(r => { if (!CE.waiters.has(name)) CE.waiters.set(name, []); CE.waiters.get(name).push(r); }); },
  upgrade(root) { for (const el of [root, ...descendants(root)]) if (el.nodeType === 1 && CE.defs.has(el.localName)) ceUpgrade(el, CE.defs.get(el.localName)); },
};
// HTMLIFrameElement and friends: `x instanceof HTMLIFrameElement` must test the tag (style-loader and others branch on it)
const TAGGED = { Script: 'script', Input: 'input', Div: 'div', Span: 'span', Anchor: 'a', Image: 'img', Form: 'form', Button: 'button', Select: 'select', TextArea: 'textarea', Canvas: 'canvas', Style: 'style', Link: 'link', Body: 'body', Head: 'head', Html: 'html', Table: 'table', IFrame: 'iframe', Option: 'option', Label: 'label', Paragraph: 'p', Heading: 'h1 h2 h3 h4 h5 h6', UList: 'ul', OList: 'ol', LI: 'li', Media: 'audio video', Video: 'video', Audio: 'audio', Template: 'template' };
for (const [n, tags] of Object.entries(TAGGED)) { const T = tags.split(' '); g['HTML' + n + 'Element'] = class extends Element { static [Symbol.hasInstance](o) { return o instanceof Element && T.includes(o.localName); } }; }
Object.defineProperty(g.HTMLScriptElement.prototype, 'noModule', { value: false, writable: true, configurable: true }); // `'noModule' in HTMLScriptElement.prototype` is how pages detect module support
g.HTMLScriptElement.supports = t => t === 'classic' || t === 'module' || t === 'importmap';
// ---- getComputedStyle: a small cascade over the page's <style> elements (external sheets are inlined by the loader) and the inline style
// shortcut: declared values are returned as written (no unit conversion, no `var()`/`calc()` evaluation, `@media` blocks are skipped), no pseudo-elements, no transitions; specificity ignores :is()/:not() arguments
const UA_NONE = new Set('head script style title meta link template noscript option datalist object iframe canvas audio video base'.split(' '));
const UA_BLOCK = new Set('html body div p h1 h2 h3 h4 h5 h6 ul ol dl dt dd form hr blockquote figure figcaption details summary address pre fieldset legend center section article header footer nav main aside menu dir'.split(' '));
const UA_DISPLAY = { li: 'list-item', table: 'table', tr: 'table-row', td: 'table-cell', th: 'table-cell', thead: 'table-header-group', tbody: 'table-row-group', tfoot: 'table-footer-group', caption: 'table-caption', img: 'inline', button: 'inline-block', input: 'inline-block', select: 'inline-block', textarea: 'inline-block' };
const INHERITED = new Set('visibility color font-size font-family font-weight font-style line-height text-align white-space cursor direction letter-spacing text-transform list-style-type word-spacing'.split(' '));
const splitTop = (s, ch) => { const out = []; let d = 0, q = '', cur = ''; for (const c of s) { if (q) { if (c === q) q = ''; } else if (c === '"' || c === "'") q = c; else if (c === '(' || c === '[') d++; else if (c === ')' || c === ']') d--; if (c === ch && !d && !q) { out.push(cur); cur = ''; } else cur += c; } out.push(cur); return out; };
const parseDecls = t => { const o = []; for (const d of splitTop(t, ';')) { const i = d.indexOf(':'); if (i < 1) continue; let v = d.slice(i + 1).trim(); const imp = /!\s*important\s*$/i.test(v); if (imp) v = v.replace(/!\s*important\s*$/i, '').trim(); o.push([d.slice(0, i).trim().toLowerCase(), v, imp]); } return o; };
const specificity = s => { s = s.replace(/\[[^\]]*\]/g, ' [a] ').replace(/::?(?:before|after|first-line|first-letter)/g, ''); const ids = (s.match(/#[\w-]+/g) || []).length, cls = (s.match(/\.[\w-]+|\[a\]|:(?!:)[\w-]+/g) || []).length, tags = (s.replace(/[#.:][\w-]+/g, ' ').match(/(?:^|[\s>+~])[a-zA-Z][\w-]*/g) || []).length; return ids * 10000 + cls * 100 + tags; };
let sheetKey = null, sheetRules = [], sheetCalls = 0;
// the rightmost compound of a selector must match the element itself: remember its id, classes and tag to skip most rules cheaply
const need = s => { const last = s.split(/[\s>+~]+/).filter(Boolean).pop() || ''; const m = last.replace(/\[[^\]]*\]|:[\w-]+(\([^)]*\))?/g, ''); return { id: (m.match(/#([\w-]+)/) || [])[1], cls: (m.match(/\.[\w-]+/g) || []).map(c => c.slice(1)), tag: (m.match(/^[a-zA-Z][\w-]*/) || [])[0] }; };
const rulesOfPage = () => {
  // the <style> scan is O(page): redo it only every 32 calls (a style added in between shows up a little late)
  if (sheetKey !== null && ++sheetCalls % 32) return sheetRules;
  const styles = descendants(document).filter(e => e.localName === 'style'), key = styles.map(e => e.textContent.length).join(',') + ':' + styles.length;
  if (key === sheetKey) return sheetRules;
  sheetKey = key; sheetRules = []; let order = 0;
  for (const st of styles) {
    const css = st.textContent.replace(/\/\*[\s\S]*?\*\//g, ''); let i = 0;
    while (i < css.length) {
      const o = css.indexOf('{', i); if (o < 0) break;
      const head = css.slice(i, o).trim(); let d = 1, j = o + 1; while (j < css.length && d) { if (css[j] === '{') d++; else if (css[j] === '}') d--; j++; }
      if (!head.startsWith('@')) for (const sel of splitTop(head, ',')) { const s = sel.trim(); if (s) sheetRules.push({ sel: s, spec: specificity(s), order: order++, decls: parseDecls(css.slice(o + 1, j - 1)), need: need(s) }); }
      i = j;
    }
  }
  return sheetRules;
};
const couldMatch = (el, n) => (!n.tag || n.tag === '*' || n.tag.toLowerCase() === el.localName) && (!n.id || el._a.id === n.id) && n.cls.every(c => (' ' + (el._a.class || '') + ' ').includes(' ' + c + ' '));
const computedOf = (el, memo) => {
  if (memo.has(el)) return memo.get(el);
  const own = new Map(); memo.set(el, own);
  if (el.nodeType === 1) {
    const tag = el.localName;
    own.set('display', el._a.hidden !== undefined ? 'none' : UA_NONE.has(tag) ? 'none' : UA_BLOCK.has(tag) ? 'block' : UA_DISPLAY[tag] || 'inline');
    const hits = [];
    for (const r of rulesOfPage()) { if (!couldMatch(el, r.need)) continue; let ok = false; try { ok = matchesSel(el, r.sel); } catch (e) {} if (ok) hits.push(r); }
    hits.sort((a, b) => a.spec - b.spec || a.order - b.order);
    const imp = [];
    for (const r of hits) for (const [k, v, i] of r.decls) { if (i) imp.push([k, v]); else own.set(k, v); }
    for (const [k, v] of parseDecls(el._a.style || '')) own.set(k, v);
    for (const [k, v] of imp) own.set(k, v);
  }
  const pm = el.parentNode && el.parentNode.nodeType === 1 ? computedOf(el.parentNode, memo) : null;
  if (pm) for (const k of INHERITED) if ((!own.has(k) || own.get(k) === 'inherit') && pm.has(k)) own.set(k, pm.get(k));
  return own;
};
const DEFAULT_STYLE = { visibility: 'visible', opacity: '1', position: 'static', color: 'rgb(0, 0, 0)', 'font-size': '16px', 'font-weight': '400', 'font-style': 'normal', 'text-align': 'start', float: 'none', overflow: 'visible', 'z-index': 'auto', 'box-sizing': 'content-box', 'white-space': 'normal', 'pointer-events': 'auto' };
const computedStyle = (el, pseudo) => {
  const m = pseudo ? new Map() : computedOf(el, new Map());
  const get = name => {
    const n = String(name).replace(/[A-Z]/g, c => '-' + c.toLowerCase());
    if (m.has(n)) return m.get(n);
    if ((n === 'width' || n === 'height') && el._box) return el._box[n === 'width' ? 2 : 3] + 'px';
    return n in DEFAULT_STYLE ? DEFAULT_STYLE[n] : '';
  };
  return new Proxy({}, { get: (t, k) => k === 'getPropertyValue' ? get : k === 'getPropertyPriority' ? () => '' : k === 'length' ? m.size : k === 'cssText' ? [...m].map(([a, b]) => a + ': ' + b + ';').join(' ') : typeof k === 'symbol' || k === 'then' ? undefined : get(k) });
};
Object.assign(g, {
  window: g, self: g, top: g, parent: g, frames: g, globalThis: g, document, Node, Element, HTMLElement, Text, DocumentFragment, Document,
  Event, CustomEvent, EventTarget, MutationObserver: Observer, IntersectionObserver: IntersectionObs, ResizeObserver: ResizeObs,
  navigator: { userAgent: 'Mozilla/5.0 chrome-rs/0.1', language: 'en-US', languages: ['en-US'], platform: 'Win32', cookieEnabled: true, onLine: true, vendor: '' },
  console: { log: noop, warn: m => __log('warn: ' + m), error: (...a) => __log('error: ' + a.map(x => x && (x.message ? x.message + ' @ ' + x.stack : x.stack) || x).join(' ')), info: noop, debug: noop, trace: noop, group: noop, groupEnd: noop, table: noop, time: noop, timeEnd: noop, assert: noop },
  localStorage: store(lsData), sessionStorage: store(),
  innerWidth: 1000, innerHeight: 720, outerWidth: 1000, outerHeight: 720, devicePixelRatio: 1, scrollX: 0, scrollY: 0, pageXOffset: 0, pageYOffset: 0,
  scrollTo: noop, scrollBy: noop, focus: noop, blur: noop, alert: noop, confirm: () => false, prompt: () => null, open: () => null, close: noop, print: noop,
  getComputedStyle: computedStyle,
  matchMedia: q => ({ matches: false, media: String(q), addListener: noop, removeListener: noop, addEventListener: noop, removeEventListener: noop }),
  requestAnimationFrame: f => g.setTimeout(f, 16), cancelAnimationFrame: id => g.clearTimeout(id),
  requestIdleCallback: f => g.setTimeout(f, 1), cancelIdleCallback: id => g.clearTimeout(id),
  setTimeout: (f, d, ...a) => { timers.push({ id: ++tid, f, t: now + (+d || 0), a }); return tid; },
  setInterval: (f, d, ...a) => { const e = Math.max(+d || 0, 1); timers.push({ id: ++tid, f, t: now + e, a, every: e, n: 0 }); return tid; },
  clearTimeout: id => { cleared.add(id); timers = timers.filter(x => x.id !== id); },
  clearInterval: id => { cleared.add(id); timers = timers.filter(x => x.id !== id); },
});

// ---- URL, encoding, forms, networking, history (M4 web APIs on top of the synchronous natives __fetch / __resolve)
const dec = s => { try { return decodeURIComponent(s.replace(/\+/g, ' ')); } catch (e) { return s; } };
const encq = s => encodeURIComponent(s).replace(/%20/g, '+');
class URLSearchParams {
  constructor(init) {
    this._p = [];
    if (init instanceof URLSearchParams) this._p = init._p.map(x => x.slice());
    else if (typeof init === 'string') { for (const kv of init.replace(/^\?/, '').split('&')) if (kv) { const i = kv.indexOf('='); this._p.push([dec(i < 0 ? kv : kv.slice(0, i)), dec(i < 0 ? '' : kv.slice(i + 1))]); } }
    else if (init && typeof init[Symbol.iterator] === 'function') for (const [k, v] of init) this._p.push([String(k), String(v)]);
    else if (init) for (const k of Object.keys(init)) this._p.push([k, String(init[k])]);
  }
  _sync() { if (this._u) { const q = this.toString(); this._u.search = q ? '?' + q : ''; } }
  append(k, v) { this._p.push([String(k), String(v)]); this._sync(); }
  delete(k) { this._p = this._p.filter(x => x[0] !== String(k)); this._sync(); }
  get(k) { const e = this._p.find(x => x[0] === String(k)); return e ? e[1] : null; }
  getAll(k) { return this._p.filter(x => x[0] === String(k)).map(x => x[1]); }
  has(k) { return this._p.some(x => x[0] === String(k)); }
  set(k, v) { k = String(k); const i = this._p.findIndex(x => x[0] === k); if (i < 0) this._p.push([k, String(v)]); else { this._p[i][1] = String(v); this._p = this._p.filter((x, j) => x[0] !== k || j === i); } this._sync(); }
  sort() { this._p.sort((a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0); this._sync(); }
  forEach(f, t) { for (const [k, v] of this._p) f.call(t, v, k, this); }
  keys() { return this._p.map(x => x[0])[Symbol.iterator](); }
  values() { return this._p.map(x => x[1])[Symbol.iterator](); }
  entries() { return this._p.map(x => [x[0], x[1]])[Symbol.iterator](); }
  [Symbol.iterator]() { return this.entries(); }
  get size() { return this._p.length; }
  toString() { return this._p.map(([k, v]) => encq(k) + '=' + encq(v)).join('&'); }
}
class URL {
  constructor(u, base) {
    u = String(u);
    if (!/^[a-z][a-z0-9+.-]*:/i.test(u)) { if (base === undefined) throw new TypeError('Invalid URL: ' + u); u = __resolve(String(base instanceof URL ? base.href : base), u); }
    const m = /^([a-z][a-z0-9+.-]*:)(?:\/\/(?:([^:@\/?#]*)(?::([^@\/?#]*))?@)?([^\/?#:]*)(?::(\d+))?)?([^?#]*)(\?[^#]*)?(#.*)?$/i.exec(u);
    if (!m) throw new TypeError('Invalid URL: ' + u);
    this.protocol = m[1].toLowerCase(); this.username = m[2] || ''; this.password = m[3] || ''; this.hostname = (m[4] || '').toLowerCase(); this.port = m[5] || '';
    this.pathname = m[6] || (m[4] !== undefined ? '/' : ''); this.search = m[7] && m[7] !== '?' ? m[7] : ''; this.hash = m[8] && m[8] !== '#' ? m[8] : '';
    this.searchParams = new URLSearchParams(this.search); this.searchParams._u = this;
  }
  get host() { return this.hostname + (this.port ? ':' + this.port : ''); }
  get origin() { return /^https?:$/.test(this.protocol) ? this.protocol + '//' + this.host : 'null'; }
  get href() { return this.protocol + (this.hostname || this.protocol === 'file:' ? '//' : '') + (this.username ? this.username + (this.password ? ':' + this.password : '') + '@' : '') + this.host + this.pathname + this.search + this.hash; }
  toString() { return this.href; } toJSON() { return this.href; }
  static canParse(u, b) { try { new URL(u, b); return true; } catch (e) { return false; } }
}
const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
const btoa = s => {
  s = String(s); let o = '';
  for (let i = 0; i < s.length; i += 3) {
    const a = s.charCodeAt(i), b = s.charCodeAt(i + 1), c = s.charCodeAt(i + 2);
    if (a > 255 || b > 255 || c > 255) throw new Error('InvalidCharacterError');
    o += B64[a >> 2] + B64[(a & 3) << 4 | (b || 0) >> 4] + (i + 1 < s.length ? B64[(b & 15) << 2 | (c || 0) >> 6] : '=') + (i + 2 < s.length ? B64[c & 63] : '=');
  }
  return o;
};
const atob = s => {
  s = String(s).replace(/[\s=]+/g, ''); let o = '', acc = 0, bits = 0;
  for (const ch of s) { const v = B64.indexOf(ch); if (v < 0) throw new Error('InvalidCharacterError'); acc = acc << 6 | v; bits += 6; if (bits >= 8) { bits -= 8; o += String.fromCharCode(acc >> bits & 255); } }
  return o;
};
class TextEncoder {
  get encoding() { return 'utf-8'; }
  encode(s) { const e = unescape(encodeURIComponent(String(s === undefined ? '' : s))); const a = new Uint8Array(e.length); for (let i = 0; i < e.length; i++) a[i] = e.charCodeAt(i); return a; }
}
class TextDecoder {
  constructor(l) { this.encoding = String(l || 'utf-8').toLowerCase(); }
  decode(b) {
    if (!b) return '';
    const u = b instanceof ArrayBuffer ? new Uint8Array(b) : new Uint8Array(b.buffer || b, b.byteOffset || 0, b.byteLength === undefined ? b.length : b.byteLength);
    let s = ''; for (let i = 0; i < u.length; i++) s += String.fromCharCode(u[i]);
    try { return decodeURIComponent(escape(s)); } catch (e) { return s; }
  }
}
class Blob {
  constructor(parts, o) { this._t = (parts || []).map(p => typeof p === 'string' ? p : p instanceof Blob ? p._t : p && p.byteLength !== undefined ? new TextDecoder().decode(p) : String(p)).join(''); this.type = (o && o.type) || ''; }
  get size() { return unescape(encodeURIComponent(this._t)).length; }
  text() { return Promise.resolve(this._t); }
  arrayBuffer() { return Promise.resolve(new TextEncoder().encode(this._t).buffer); }
}
class FormData {
  constructor(form) {
    this._e = [];
    if (form && form.nodeType === 1) for (const e of descendants(form)) {
      const n = e._a.name, t = (e._a.type || '').toLowerCase();
      if (!n || 'disabled' in e._a) continue;
      if (e.localName === 'input') { if (['submit', 'button', 'reset', 'image', 'file'].includes(t)) continue; if ((t === 'checkbox' || t === 'radio') && !('checked' in e._a)) continue; this._e.push([n, e._a.value !== undefined ? e._a.value : (t === 'checkbox' || t === 'radio' ? 'on' : '')]); }
      else if (e.localName === 'textarea') this._e.push([n, e.textContent]);
      else if (e.localName === 'select') { const o = descendants(e).filter(x => x.localName === 'option'); const s = o.find(x => 'selected' in x._a) || o[0]; if (s) this._e.push([n, s._a.value !== undefined ? s._a.value : s.textContent.trim()]); }
    }
  }
  append(k, v) { this._e.push([String(k), v instanceof Blob ? v : String(v)]); }
  delete(k) { this._e = this._e.filter(x => x[0] !== String(k)); }
  get(k) { const e = this._e.find(x => x[0] === String(k)); return e ? e[1] : null; }
  getAll(k) { return this._e.filter(x => x[0] === String(k)).map(x => x[1]); }
  has(k) { return this._e.some(x => x[0] === String(k)); }
  set(k, v) { k = String(k); const i = this._e.findIndex(x => x[0] === k); if (i < 0) this._e.push([k, String(v)]); else { this._e[i][1] = String(v); this._e = this._e.filter((x, j) => x[0] !== k || j === i); } }
  forEach(f, t) { for (const [k, v] of this._e) f.call(t, v, k, this); }
  keys() { return this._e.map(x => x[0])[Symbol.iterator](); }
  values() { return this._e.map(x => x[1])[Symbol.iterator](); }
  entries() { return this._e.map(x => [x[0], x[1]])[Symbol.iterator](); }
  [Symbol.iterator]() { return this.entries(); }
}
class Headers {
  constructor(init) { this._h = {}; if (init instanceof Headers) init = init._h; if (Array.isArray(init)) for (const [k, v] of init) this.append(k, v); else if (init) for (const k of Object.keys(init)) this.append(k, init[k]); }
  append(k, v) { k = String(k).toLowerCase(); this._h[k] = k in this._h ? this._h[k] + ', ' + v : String(v); }
  set(k, v) { this._h[String(k).toLowerCase()] = String(v); }
  get(k) { k = String(k).toLowerCase(); return k in this._h ? this._h[k] : null; }
  has(k) { return String(k).toLowerCase() in this._h; }
  delete(k) { delete this._h[String(k).toLowerCase()]; }
  forEach(f, t) { for (const k of Object.keys(this._h)) f.call(t, this._h[k], k, this); }
  entries() { return Object.entries(this._h)[Symbol.iterator](); }
  keys() { return Object.keys(this._h)[Symbol.iterator](); }
  values() { return Object.values(this._h)[Symbol.iterator](); }
  [Symbol.iterator]() { return this.entries(); }
}
// request body -> [content-type, text]; FormData goes out as multipart/form-data (text fields only)
const prep = (body, ctype) => {
  if (body === undefined || body === null) return [ctype || '', ''];
  if (body instanceof URLSearchParams) return ['application/x-www-form-urlencoded;charset=UTF-8', body.toString()];
  if (body instanceof FormData) {
    const b = '----chromers' + Math.floor(Math.random() * 1e9);
    return ['multipart/form-data; boundary=' + b, body._e.map(([k, v]) => '--' + b + '\r\nContent-Disposition: form-data; name="' + k.replace(/"/g, '%22') + '"\r\n\r\n' + (v instanceof Blob ? v._t : v) + '\r\n').join('') + '--' + b + '--\r\n'];
  }
  if (body instanceof Blob) return [ctype || body.type, body._t];
  return [ctype || (typeof body === 'string' ? 'text/plain;charset=UTF-8' : ''), typeof body === 'string' ? body : String(body)];
};
const netFetch = (url, method, body, headers) => {
  const h = new Headers(headers), [ct, text] = prep(body, h.get('content-type'));
  return JSON.parse(__fetch(String(url), method, method === 'GET' || method === 'HEAD' ? '' : text, ct));
};
class Response {
  constructor(r, url) { this.status = r.status; this.ok = r.status >= 200 && r.status < 300; this.statusText = ''; this.url = url || ''; this.type = 'basic'; this.redirected = false; this.headers = new Headers(r.ctype ? { 'content-type': r.ctype } : {}); this._b = r.body || ''; this.bodyUsed = false; }
  text() { this.bodyUsed = true; return Promise.resolve(this._b); }
  json() { this.bodyUsed = true; return Promise.resolve().then(() => JSON.parse(this._b)); }
  blob() { return Promise.resolve(new Blob([this._b], { type: this.headers.get('content-type') || '' })); }
  arrayBuffer() { return Promise.resolve(new TextEncoder().encode(this._b).buffer); }
  clone() { return this; }
}
class AbortSignal extends EventTarget { constructor() { super(); this.aborted = false; this.reason = undefined; } throwIfAborted() { if (this.aborted) throw this.reason; } }
class AbortController { constructor() { this.signal = new AbortSignal(); } abort(r) { if (this.signal.aborted) return; this.signal.aborted = true; this.signal.reason = r || new Error('AbortError'); this.signal.dispatchEvent(new Event('abort')); } }
class Hist {
  constructor() { this.entries = [{ s: null, u: location.href }]; this.i = 0; this.scrollRestoration = 'auto'; }
  get length() { return this.entries.length; } get state() { return this.entries[this.i].s; }
  pushState(s, t, u) { if (u !== undefined && u !== null) setLoc(__resolve(location.href, String(u))); this.entries.length = this.i + 1; this.entries.push({ s, u: location.href }); this.i++; }
  replaceState(s, t, u) { if (u !== undefined && u !== null) setLoc(__resolve(location.href, String(u))); this.entries[this.i] = { s, u: location.href }; }
  go(d) { const j = this.i + (d | 0); if (j < 0 || j >= this.entries.length || !d) return; this.i = j; setLoc(this.entries[j].u); g.setTimeout(() => g.dispatchEvent(new g.PopStateEvent('popstate', { state: this.entries[j].s })), 0); }
  back() { this.go(-1); } forward() { this.go(1); }
}
for (const n of ['MouseEvent', 'FocusEvent', 'InputEvent', 'PointerEvent', 'TouchEvent', 'WheelEvent', 'ErrorEvent', 'MessageEvent', 'PopStateEvent', 'HashChangeEvent', 'StorageEvent', 'ProgressEvent', 'SubmitEvent'])
  g[n] = class extends Event { constructor(t, i) { super(t, i); Object.assign(this, i || {}); } };
// the UI sends `key` only: derive the legacy codes (`e.which === 13` is still how jQuery/Backbone apps spot Enter)
const KEYCODES = { Enter: 13, Escape: 27, Backspace: 8, Tab: 9, ' ': 32, Delete: 46, ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Home: 36, End: 35, PageUp: 33, PageDown: 34 };
g.KeyboardEvent = class KeyboardEvent extends Event {
  constructor(t, i) {
    super(t, i); Object.assign(this, i || {});
    const k = this.key === undefined ? '' : String(this.key);
    if (this.keyCode === undefined) this.keyCode = KEYCODES[k] || (k.length === 1 ? k.toUpperCase().charCodeAt(0) : 0);
    if (this.which === undefined) this.which = this.keyCode;
    if (this.code === undefined) this.code = k === 'Enter' ? 'Enter' : k.length === 1 ? (/[a-z]/i.test(k) ? 'Key' + k.toUpperCase() : /\d/.test(k) ? 'Digit' + k : '') : k;
  }
};
class WS extends EventTarget { // no sockets: connections fail asynchronously so pages fall back instead of throwing
  constructor(u) { super(); this.url = String(u); this.readyState = 0; this.bufferedAmount = 0; g.setTimeout(() => { this.readyState = 3; for (const t of ['error', 'close']) { const e = new Event(t); if (typeof this['on' + t] === 'function') this['on' + t](e); this.dispatchEvent(e); } }, 0); }
  send() {} close() { this.readyState = 3; }
}
Object.assign(g, {
  URL, URLSearchParams, btoa, atob, TextEncoder, TextDecoder, Blob, FormData, Headers, Response, AbortController, AbortSignal, WebSocket: WS, EventSource: WS, history: new Hist(),
  performance: { now: () => now + (perfT += 0.01), timeOrigin: 0, mark: noop, measure: noop, getEntriesByName: () => [], getEntriesByType: () => [], clearMarks: noop, clearMeasures: noop },
  queueMicrotask: f => { Promise.resolve().then(f).catch(noop); },
  structuredClone: v => v === undefined ? v : JSON.parse(JSON.stringify(v)),
  crypto: { getRandomValues: a => { const m = Math.pow(256, a.BYTES_PER_ELEMENT || 1); for (let i = 0; i < a.length; i++) a[i] = Math.floor(Math.random() * m); return a; }, randomUUID: () => 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, c => { const r = Math.random() * 16 | 0; return (c === 'x' ? r : r & 3 | 8).toString(16); }), subtle: {} },
  customElements: customElementsApi,
  CSS: { supports: () => false, escape: s => String(s).replace(/[^\w-]/g, c => '\\' + c) },
  getSelection: () => ({ rangeCount: 0, toString: () => '', removeAllRanges: noop, addRange: noop }),
  Image: function () { return document.createElement('img'); },
  fetch: (u, o) => new Promise((res, rej) => {
    o = o || {};
    const req = u instanceof Object && u.url ? u : null, url = String(req ? req.url : u), method = String(o.method || (req && req.method) || 'GET').toUpperCase();
    if (o.signal && o.signal.aborted) return rej(o.signal.reason);
    const r = netFetch(url, method, o.body, o.headers);
    if (!r.status) return rej(new TypeError('Failed to fetch'));
    res(new Response(r, url));
  }),
  XMLHttpRequest: class extends EventTarget {
    constructor() { super(); this.readyState = 0; this.status = 0; this.statusText = ''; this.responseText = ''; this.response = ''; this.responseType = ''; this.responseURL = ''; this._h = new Headers(); this.timeout = 0; this.withCredentials = false; this.upload = new EventTarget(); }
    open(m, u) { this._m = String(m).toUpperCase(); this._u = String(u); this.readyState = 1; }
    setRequestHeader(k, v) { this._h.append(k, v); } abort() { this._abort = true; }
    getResponseHeader(k) { return this._rh ? this._rh.get(k) : null; }
    getAllResponseHeaders() { let s = ''; if (this._rh) this._rh.forEach((v, k) => { s += k + ': ' + v + '\r\n'; }); return s; }
    overrideMimeType() {}
    send(body) {
      const r = netFetch(this._u, this._m, body, this._h);
      g.setTimeout(() => {
        if (this._abort) return;
        this.status = r.status; this.responseText = r.body || ''; this.responseURL = this._u; this._rh = new Headers(r.ctype ? { 'content-type': r.ctype } : {}); this.readyState = 4;
        try { this.response = this.responseType === 'json' ? JSON.parse(this.responseText) : this.responseText; } catch (e) { this.response = null; }
        const fire = t => { const e = new Event(t); if (typeof this['on' + t] === 'function') { try { this['on' + t].call(this, e); } catch (x) {} } this.dispatchEvent(e); };
        fire('readystatechange'); fire(r.status ? 'load' : 'error'); fire('loadend');
      }, 0);
    }
  },
});
g.navigator.sendBeacon = () => true;
// interface objects scripts test with instanceof or feature checks; the DOM itself hands out plain arrays and elements
class File extends Blob { constructor(parts, name, o) { super(parts, o); this.name = String(name); this.lastModified = (o && o.lastModified) || Date.now(); } }
Object.assign(g, {
  File, NodeList: class NodeList extends Array {}, HTMLCollection: class HTMLCollection extends Array {}, ShadowRoot: class ShadowRoot extends DocumentFragment {
    constructor() { super(); this.mode = 'open'; this.adoptedStyleSheets = []; this.activeElement = null; }
    get innerHTML() { return this.childNodes.map(ser).join(''); }
    set innerHTML(h) { this.childNodes.forEach(c => c.parentNode = null); this.childNodes = []; for (const c of fromJSON(JSON.parse(__parse(String(h))))) this.appendChild(c); }
    getRootNode() { return this; }
  },
  SVGElement: Element, Storage: class Storage {},
  screen: { width: 1000, height: 720, availWidth: 1000, availHeight: 720, colorDepth: 24, pixelDepth: 24, orientation: { type: 'landscape-primary', angle: 0 } },
});

// ---- more of the platform surface that real scripts probe. Where nothing is rendered behind an API it is a stub that answers like an empty page.
const def = (o, props) => { for (const k of Object.keys(props)) if (!(k in o)) Object.defineProperty(o, k, Object.getOwnPropertyDescriptor(props, k)); };
const isLink = e => e.localName === 'a' || e.localName === 'area';
const linkPart = (e, k) => { if (!isLink(e)) return undefined; try { return new URL(e.href)[k]; } catch (x) { return ''; } };
const toNode = n => typeof n === 'string' ? new Text(n) : n;
const fragmentOf = h => { const f = new DocumentFragment(); for (const c of fromJSON(JSON.parse(__parse(String(h))))) f.appendChild(c); return f; };
def(Element.prototype, {
  insertAdjacentElement(w, n) { w = String(w).toLowerCase(); if (w === 'beforebegin') { if (this.parentNode) this.parentNode.insertBefore(n, this); } else if (w === 'afterbegin') this.insertBefore(n, this.childNodes[0] || null); else if (w === 'beforeend') this.appendChild(n); else if (w === 'afterend' && this.parentNode) this.parentNode.insertBefore(n, this.nextSibling); return n; },
  insertAdjacentText(w, t) { this.insertAdjacentElement(w, new Text(t)); },
  getAttributeNames() { return Object.keys(this._a); }, hasAttributes() { return Object.keys(this._a).length > 0; },
  replaceWith(...ns) { const p = this.parentNode; if (!p) return; for (const n of ns) p.insertBefore(toNode(n), this); p.removeChild(this); },
  replaceChildren(...ns) { for (const c of this.childNodes.slice()) this.removeChild(c); for (const n of ns) this.appendChild(toNode(n)); },
  scrollTo() {}, scroll() {}, normalize() {}, getAnimations() { return []; }, requestFullscreen() { return Promise.resolve(); }, setPointerCapture() {}, releasePointerCapture() {},
  isSameNode(n) { return n === this; }, isEqualNode(n) { return !!n && ser(n) === ser(this); },
  compareDocumentPosition(n) { if (n === this) return 0; if (this.contains(n)) return 20; if (n.contains && n.contains(this)) return 10; const all = descendants(document); return all.indexOf(n) > all.indexOf(this) ? 4 : 2; },
  attachShadow(o) { if (this._sr) throw new Error('NotSupportedError: shadow root already attached'); const s = new ShadowRoot(); s.host = this; s.mode = (o && o.mode) || 'open'; this._sr = s; return s; },
  get shadowRoot() { return this._sr && this._sr.mode === 'open' ? this._sr : null; },
  assignedNodes() { if (this.localName !== 'slot') return []; let r = this; while (r.parentNode) r = r.parentNode; return r.host ? slotted(this, r.host) : []; },
  assignedElements() { return this.assignedNodes().filter(n => n.nodeType === 1); },
  getAttributeNS(ns, n) { return this.getAttribute(n); }, setAttributeNS(ns, n, v) { this.setAttribute(n, v); }, removeAttributeNS(ns, n) { this.removeAttribute(n); },
  get tabIndex() { const v = this._a.tabindex; return v === undefined ? (/^(a|button|input|select|textarea)$/.test(this.localName) ? 0 : -1) : +v; }, set tabIndex(v) { this._a.tabindex = String(v); },
  get offsetParent() { return this.parentNode && this.parentNode.nodeType === 1 ? this.parentNode : null; },
  get namespaceURI() { return this._ns || 'http://www.w3.org/1999/xhtml'; },
  // <template>.content: a fragment holding clones of the template's children (templates are never laid out)
  set content(v) { this._a.content = String(v); },
  get content() { if (this.localName !== 'template') return this._a.content || ''; if (!this._content) { this._content = new DocumentFragment(); for (const c of this.childNodes) this._content.appendChild(c.cloneNode(true, true)); } return this._content; },
  // forms: `form.elements['name']` and `control.form`
  get elements() { if (this.localName !== 'form' && this.localName !== 'fieldset') return undefined; const l = descendants(this).filter(e => /^(input|select|textarea|button|fieldset|output|object)$/.test(e.localName) && e._a.type !== 'image'); for (const e of l) for (const k of [e._a.name, e._a.id]) if (k && !(k in l)) l[k] = e; l.namedItem = n => l[n] || null; l.item = i => l[i] || null; return l; },
  get form() { if (!/^(input|select|textarea|button|fieldset|output|object|label)$/.test(this.localName)) return undefined; for (let p = this.parentNode; p; p = p.parentNode) if (p.localName === 'form') return p; return null; },
  // links: the URL parts of <a href> / <area href>, resolved against the page like a browser does
  get pathname() { return linkPart(this, 'pathname'); }, get search() { return linkPart(this, 'search'); }, get hash() { return linkPart(this, 'hash'); },
  get host() { return linkPart(this, 'host'); }, get hostname() { return linkPart(this, 'hostname'); }, get port() { return linkPart(this, 'port'); }, get protocol() { return linkPart(this, 'protocol'); }, get origin() { return linkPart(this, 'origin'); },
  // <select>/<option>: the one selected state is the `selected` attribute (React's select handling reads `select.options.length`)
  get options() { if (this.localName !== 'select' && this.localName !== 'datalist') return undefined; const o = descendants(this).filter(x => x.localName === 'option'); o.item = i => o[i] || null; return o; },
  get selectedIndex() { if (this.localName !== 'select') return undefined; const o = this.options, i = o.findIndex(x => 'selected' in x._a); return i >= 0 ? i : o.length ? 0 : -1; },
  set selectedIndex(v) { this.options.forEach((x, i) => { if (i === v) x._a.selected = ''; else delete x._a.selected; }); },
  get selectedOptions() { if (this.localName !== 'select') return undefined; const o = this.options, i = this.selectedIndex; return i >= 0 ? [o[i]] : []; },
  get defaultSelected() { return 'selected' in this._a; }, set defaultSelected(v) { if (v) this._a.selected = ''; else delete this._a.selected; },
  get index() { let s = this.parentNode; while (s && s.localName !== 'select') s = s.parentNode; return s ? s.options.indexOf(this) : 0; },
  get text() { return this.textContent; }, set text(v) { this.textContent = v; },
  get clientTop() { return 0; }, get clientLeft() { return 0; }, get scrollLeft() { return 0; }, set scrollLeft(v) {},
  get dir() { return this._a.dir || ''; }, set dir(v) { this._a.dir = String(v); }, get slot() { return this._a.slot || ''; }, set slot(v) { this._a.slot = String(v); },
  get draggable() { return this._a.draggable === 'true'; }, set draggable(v) { this._a.draggable = String(!!v); },
  get contentEditable() { return this._a.contenteditable || 'inherit'; }, set contentEditable(v) { this._a.contenteditable = String(v); },
  get isContentEditable() { return this._a.contenteditable === 'true' || this._a.contenteditable === ''; },
});
const fontSet = { ready: Promise.resolve(), status: 'loaded', size: 0, load: () => Promise.resolve([]), check: () => true, forEach: noop, add: noop, delete: noop, has: () => false, addEventListener: noop, removeEventListener: noop, [Symbol.iterator]: function* () {} };
class CSSStyleSheet { constructor() { this.cssRules = []; this.rules = this.cssRules; this.disabled = false; this.media = { mediaText: '' }; this._text = ''; } insertRule(r) { this._text += ' ' + r; return 0; } deleteRule() {} addRule() { return 0; } replace(t) { this._text = String(t); return Promise.resolve(this); } replaceSync(t) { this._text = String(t); } }
const NodeFilter = { FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3, SHOW_ALL: 0xFFFFFFFF, SHOW_ELEMENT: 1, SHOW_ATTRIBUTE: 2, SHOW_TEXT: 4, SHOW_CDATA_SECTION: 8, SHOW_PROCESSING_INSTRUCTION: 64, SHOW_COMMENT: 128, SHOW_DOCUMENT: 256, SHOW_DOCUMENT_TYPE: 512, SHOW_DOCUMENT_FRAGMENT: 1024 };
class TreeWalker {
  // walks the live tree in document order from `currentNode` (Lit moves currentNode into a template before walking)
  constructor(root, what, filter) { this.root = root; this.currentNode = root; this.whatToShow = what === undefined ? 0xFFFFFFFF : what >>> 0; this.filter = filter || null; }
  _ok(n) { if (!(this.whatToShow & (1 << (n.nodeType - 1)))) return false; const f = this.filter; if (!f) return true; const r = typeof f === 'function' ? f(n) : f.acceptNode(n); return r === 1 || r === true; }
  _next(n) { if (n.childNodes && n.childNodes.length) return n.childNodes[0]; for (; n && n !== this.root; n = n.parentNode) { const s = n.nextSibling; if (s) return s; } return null; }
  _prev(n) { if (n === this.root) return null; let s = n.previousSibling; if (!s) return n.parentNode === this.root ? null : n.parentNode; while (s.childNodes && s.childNodes.length) s = s.childNodes[s.childNodes.length - 1]; return s; }
  nextNode() { let n = this.currentNode; while ((n = this._next(n))) if (this._ok(n)) return this.currentNode = n; return null; }
  previousNode() { let n = this.currentNode; while ((n = this._prev(n))) if (this._ok(n)) return this.currentNode = n; return null; }
  firstChild() { const n = this.currentNode.childNodes && this.currentNode.childNodes[0]; return n && this._ok(n) ? this.currentNode = n : null; }
  parentNode() { const n = this.currentNode === this.root ? null : this.currentNode.parentNode; return n && this._ok(n) ? this.currentNode = n : null; }
}
class Range {
  constructor() { this.collapsed = true; this.startContainer = this.endContainer = document; this.startOffset = this.endOffset = 0; }
  setStart(n, o) { this.startContainer = n; this.startOffset = o; } setEnd(n, o) { this.endContainer = n; this.endOffset = o; }
  selectNode(n) { this.startContainer = this.endContainer = n.parentNode; } selectNodeContents(n) { this.startContainer = this.endContainer = n; }
  collapse() {} detach() {} cloneRange() { return Object.assign(new Range(), this); } deleteContents() {} toString() { return ''; }
  getBoundingClientRect() { return { x: 0, y: 0, left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 }; } getClientRects() { return []; }
  createContextualFragment(h) { return fragmentOf(h); }
}
def(Document.prototype, {
  get styleSheets() { return Object.assign(descendants(this).filter(e => e.localName === 'style' || (e.localName === 'link' && /stylesheet/i.test(e._a.rel || ''))).map(e => Object.assign(new CSSStyleSheet(), { ownerNode: e, href: e._a.href || null })), { item(i) { return this[i] || null; } }); },
  get scripts() { return this.getElementsByTagName('script'); }, get forms() { return this.getElementsByTagName('form'); }, get images() { return this.getElementsByTagName('img'); }, get all() { return this.getElementsByTagName('*'); },
  get links() { return descendants(this).filter(e => (e.localName === 'a' || e.localName === 'area') && 'href' in e._a); },
  get compatMode() { return 'CSS1Compat'; }, get doctype() { return { name: 'html', nodeType: 10, publicId: '', systemId: '' }; }, get fonts() { return fontSet; },
  get activeElement() { return this.body; }, get domain() { return loc.hostname; }, get lastModified() { return ''; }, get URL() { return loc._h; }, get documentURI() { return loc._h; }, get baseURI() { return loc._h; },
  get contentType() { return 'text/html'; }, get dir() { return ''; }, get inputEncoding() { return 'UTF-8'; },
  createRange() { return new Range(); }, createTreeWalker(r, w, f) { return new TreeWalker(r, w, f); }, elementFromPoint() { return this.body; }, elementsFromPoint() { return []; },
  getElementsByName(n) { return descendants(this).filter(e => e._a.name === n); }, adoptNode(n) { return n; }, importNode(n, deep) { return n.cloneNode(deep); },
  execCommand() { return false; }, queryCommandSupported() { return false; },
});
const adopted = { get adoptedStyleSheets() { return this._ass || (this._ass = []); }, set adoptedStyleSheets(v) { this._ass = Array.from(v); } };
def(Document.prototype, adopted); def(DocumentFragment.prototype, adopted);
class DOMParser { parseFromString(s) { const d = document.implementation.createHTMLDocument(''); d.body.innerHTML = String(s); return d; } }
class XMLSerializer { serializeToString(n) { return ser(n); } }
class ReadableStream { constructor() { this.locked = false; } getReader() { return { read: () => new Promise(noop), releaseLock: noop, cancel: () => Promise.resolve(), closed: new Promise(noop) }; } cancel() { return Promise.resolve(); } pipeThrough(t) { return t.readable; } pipeTo() { return Promise.resolve(); } tee() { return [this, this]; } }
class WritableStream { constructor() { this.locked = false; } getWriter() { return { write: () => Promise.resolve(), close: () => Promise.resolve(), abort: () => Promise.resolve(), releaseLock: noop, ready: Promise.resolve(), closed: Promise.resolve() }; } }
class TransformStream { constructor() { this.readable = new ReadableStream(); this.writable = new WritableStream(); } }
// message ports deliver asynchronously to the other end (schedulers such as React's use a MessageChannel)
class MessagePort extends EventTarget {
  constructor() { super(); this.onmessage = null; this._o = null; }
  postMessage(d) { const o = this._o; if (o) g.setTimeout(() => { const e = new Event('message'); e.data = d; e.target = o; if (o.onmessage) o.onmessage(e); o.dispatchEvent(e); }, 0); }
  start() {} close() { this._o = null; }
}
class MessageChannel { constructor() { this.port1 = new MessagePort(); this.port2 = new MessagePort(); this.port1._o = this.port2; this.port2._o = this.port1; } }
class BroadcastChannel extends EventTarget { constructor(n) { super(); this.name = n; } postMessage() {} close() {} }
class FileReader extends EventTarget {
  _done(r) { g.setTimeout(() => { this.result = r; this.readyState = 2; for (const t of ['load', 'loadend']) { const e = new Event(t); e.target = this; if (this['on' + t]) this['on' + t](e); this.dispatchEvent(e); } }, 0); }
  readAsText(b) { this._done(b._t); } readAsDataURL(b) { this._done('data:' + (b.type || 'application/octet-stream') + ';base64,' + btoa(unescape(encodeURIComponent(b._t)))); }
  readAsArrayBuffer(b) { this._done(new TextEncoder().encode(b._t).buffer); } abort() {}
}
class Request {
  constructor(i, o) { o = o || {}; const r = i instanceof Request ? i : null; this.url = String(r ? r.url : i); this.method = String(o.method || (r && r.method) || 'GET').toUpperCase(); this.headers = new Headers(o.headers || (r && r.headers)); this.body = o.body !== undefined ? o.body : r ? r.body : null; this.signal = o.signal || null; this.credentials = o.credentials || 'same-origin'; this.mode = o.mode || 'cors'; }
  clone() { return new Request(this); }
}
class DOMRect { constructor(x = 0, y = 0, w = 0, h = 0) { Object.assign(this, { x, y, width: w, height: h }); } get left() { return Math.min(this.x, this.x + this.width); } get right() { return Math.max(this.x, this.x + this.width); } get top() { return Math.min(this.y, this.y + this.height); } get bottom() { return Math.max(this.y, this.y + this.height); } }
class DOMPoint { constructor(x = 0, y = 0, z = 0, w = 1) { Object.assign(this, { x, y, z, w }); } }
class PerformanceObserver extends Observer { static get supportedEntryTypes() { return []; } }
const pad = n => String(n).padStart(2, '0');
// Intl: English only. Dates use the engine's local time (or UTC with timeZone: 'UTC'); there is no locale data, so other languages format like English.
const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'], DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
class DateTimeFormat {
  constructor(l, o) { this.l = (Array.isArray(l) ? l[0] : l) || 'en-US'; this.o = o || {}; }
  resolvedOptions() { return Object.assign({ locale: this.l, calendar: 'gregory', numberingSystem: 'latn', timeZone: 'UTC' }, this.o); }
  format(d) {
    d = d === undefined ? new Date() : new Date(d); let o = this.o; const u = o.timeZone === 'UTC', G = k => d[(u ? 'getUTC' : 'get') + k]();
    if (o.dateStyle || o.timeStyle) { const D = { full: { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric' }, long: { year: 'numeric', month: 'long', day: 'numeric' }, medium: { year: 'numeric', month: 'short', day: 'numeric' }, short: { year: '2-digit', month: 'numeric', day: 'numeric' } }, T = { short: { hour: 'numeric', minute: '2-digit' } }; o = Object.assign({}, o.dateStyle ? D[o.dateStyle] : {}, o.timeStyle ? Object.assign({ hour: 'numeric', minute: '2-digit' }, o.timeStyle === 'short' ? {} : { second: '2-digit' }) : {}, { hour12: o.hour12 }); }
    const has = ['weekday', 'year', 'month', 'day', 'hour', 'minute', 'second'].some(k => o[k]);
    if (!has) o = Object.assign({}, o, { year: 'numeric', month: 'numeric', day: 'numeric' });
    const parts = []; let date = '';
    if (o.year || o.month || o.day || o.weekday) {
      const y = o.year ? (o.year === '2-digit' ? pad(G('FullYear') % 100) : String(G('FullYear'))) : '', dd = o.day ? (o.day === '2-digit' ? pad(G('Date')) : String(G('Date'))) : '', wd = o.weekday ? (o.weekday === 'long' ? DAYS[G('Day')] : DAYS[G('Day')].slice(0, 3)) : '';
      if (o.month === 'long' || o.month === 'short' || o.month === 'narrow') { const m = o.month === 'long' ? MONTHS[G('Month')] : MONTHS[G('Month')].slice(0, o.month === 'short' ? 3 : 1); date = (wd ? wd + ', ' : '') + [m, dd].filter(Boolean).join(' ') + (y ? (dd ? ', ' : ' ') + y : ''); }
      else { const m = o.month ? (o.month === '2-digit' ? pad(G('Month') + 1) : String(G('Month') + 1)) : ''; date = (wd ? wd + (m || dd || y ? ', ' : '') : '') + [m, dd, y].filter(Boolean).join('/'); }
    }
    let time = '';
    if (o.hour || o.minute || o.second) {
      const h24 = G('Hours'), h12 = o.hour12 !== false && !(o.hourCycle === 'h23') && !/^(de|fr|ru|es|it|pt|ja|zh|ko)/.test(this.l), h = h12 ? (h24 % 12 || 12) : h24, t = [];
      if (o.hour) t.push(o.hour === '2-digit' || !h12 ? pad(h) : String(h)); if (o.minute) t.push(pad(G('Minutes'))); if (o.second) t.push(pad(G('Seconds')));
      time = t.join(':') + (o.hour && h12 ? (h24 < 12 ? ' AM' : ' PM') : '');
    }
    return [date, time].filter(Boolean).join(', ');
  }
  formatToParts(d) { return [{ type: 'literal', value: this.format(d) }]; }
  formatRange(a, b) { return this.format(a) + ' – ' + this.format(b); }
}
const CUR = { USD: '$', EUR: '€', GBP: '£', JPY: '¥', RUB: '₽', INR: '₹', CNY: '¥', KRW: '₩' };
class NumberFormat {
  constructor(l, o) { this.l = (Array.isArray(l) ? l[0] : l) || 'en-US'; this.o = o || {}; }
  resolvedOptions() { return Object.assign({ locale: this.l, numberingSystem: 'latn', style: 'decimal' }, this.o); }
  format(n) {
    n = Number(n); const o = this.o, cur = o.style === 'currency', pct = o.style === 'percent';
    if (!isFinite(n)) return isNaN(n) ? 'NaN' : (n < 0 ? '-∞' : '∞');
    if (pct) n *= 100;
    let min = o.minimumFractionDigits, max = o.maximumFractionDigits; if (min === undefined) min = cur ? 2 : 0; if (max === undefined) max = Math.max(min, cur ? 2 : pct ? 0 : 3);
    let suffix = ''; if (o.notation === 'compact') { for (const [v, s] of [[1e12, 'T'], [1e9, 'B'], [1e6, 'M'], [1e3, 'K']]) if (Math.abs(n) >= v) { n /= v; suffix = s; break; } max = o.maximumFractionDigits === undefined ? (Math.abs(n) < 100 && suffix ? 1 : 0) : max; min = 0; }
    let [i, f = ''] = Math.abs(n).toFixed(Math.min(max, 20)).split('.'); while (f.length > min && f.endsWith('0')) f = f.slice(0, -1);
    if (o.minimumIntegerDigits) i = i.padStart(o.minimumIntegerDigits, '0');
    if (o.useGrouping !== false && !(o.notation === 'compact')) i = i.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return (n < 0 && (i !== '0' || f) ? '-' : '') + (cur ? (CUR[o.currency] || (o.currency || '') + ' ') : '') + i + (f ? '.' + f : '') + suffix + (pct ? '%' : '');
  }
  formatToParts(n) { return [{ type: 'literal', value: this.format(n) }]; }
}
class Collator { constructor(l, o) { this.o = o || {}; } compare = (a, b) => { a = String(a); b = String(b); if (this.o.sensitivity === 'base' || this.o.sensitivity === 'accent') { a = a.toLowerCase(); b = b.toLowerCase(); } if (this.o.numeric) { const na = parseFloat(a), nb = parseFloat(b); if (!isNaN(na) && !isNaN(nb) && na !== nb) return na < nb ? -1 : 1; } return a.localeCompare(b); }; resolvedOptions() { return Object.assign({ locale: 'en-US' }, this.o); } }
class PluralRules { constructor(l, o) { this.o = o || {}; } select(n) { if (this.o.type === 'ordinal') { const a = n % 10, b = n % 100; return a === 1 && b !== 11 ? 'one' : a === 2 && b !== 12 ? 'two' : a === 3 && b !== 13 ? 'few' : 'other'; } return n === 1 ? 'one' : 'other'; } resolvedOptions() { return { locale: 'en-US' }; } }
class RelativeTimeFormat { constructor(l, o) { this.o = o || {}; } format(v, unit) { const a = Math.abs(v), s = a + ' ' + String(unit).replace(/s$/, '') + (a === 1 ? '' : 's'); return v < 0 || Object.is(v, -0) ? s + ' ago' : 'in ' + s; } }
class ListFormat { constructor(l, o) { this.o = o || {}; } format(a) { a = Array.from(a); const w = this.o.type === 'disjunction' ? 'or' : 'and'; return a.length < 3 ? a.join(' ' + w + ' ') : a.slice(0, -1).join(', ') + ', ' + w + ' ' + a[a.length - 1]; } }
class Segmenter { constructor(l, o) { this.o = o || {}; } segment(s) { const w = this.o.granularity === 'word'; const parts = w ? String(s).match(/\s+|[\p{L}\p{N}_']+|[^\s\p{L}\p{N}_']/gu) || [] : Array.from(String(s)); let i = 0; return parts.map(p => { const r = { segment: p, index: i, input: s, isWordLike: w ? /[\p{L}\p{N}]/u.test(p) : undefined }; i += p.length; return r; }); } }
const callable = C => { const F = function (...a) { return new C(...a); }; F.prototype = C.prototype; return F; }; // Intl.NumberFormat() works without `new`
const Intl = { DateTimeFormat: callable(DateTimeFormat), NumberFormat: callable(NumberFormat), Collator: callable(Collator), PluralRules, RelativeTimeFormat, ListFormat, Segmenter, getCanonicalLocales: l => [].concat(l || []), supportedValuesOf: () => [] };
Object.assign(g, {
  // no DOMParser: DOMPurify (Swagger UI) switches itself off when it is missing, but breaks on a partial one
  Comment, CharacterData: Text, XMLSerializer, Range, TreeWalker, NodeFilter, ReadableStream, WritableStream, TransformStream, MessageChannel, MessagePort, BroadcastChannel, FileReader, Request,
  DOMRect, DOMRectReadOnly: DOMRect, DOMPoint, PerformanceObserver, CSSStyleSheet, HTMLDocument: Document, XMLDocument: Document, Intl,
  Audio: function (src) { const a = document.createElement('audio'); if (src) a.setAttribute('src', src); return a; },
  Option: function (text, value) { const o = document.createElement('option'); if (text !== undefined) o.textContent = text; if (value !== undefined) o.setAttribute('value', value); return o; },
  postMessage(d) { g.setTimeout(() => { const e = new Event('message'); e.data = d; e.origin = loc.origin; e.source = g; g.dispatchEvent(e); }, 0); },
  visualViewport: { width: 1000, height: 720, scale: 1, offsetLeft: 0, offsetTop: 0, pageLeft: 0, pageTop: 0, addEventListener: noop, removeEventListener: noop },
  reportError: noop, name: '', closed: false, opener: null, frameElement: null,
});
for (const n of ['DOMTokenList', 'NamedNodeMap', 'CSSStyleDeclaration', 'CSSRule', 'StyleSheet', 'MediaQueryList', 'Attr', 'PerformanceEntry']) if (!(n in g)) g[n] = class {};
for (const n of ['UIEvent', 'CloseEvent', 'AnimationEvent', 'TransitionEvent', 'PromiseRejectionEvent']) if (!(n in g)) g[n] = class extends Event {};
Object.defineProperty(g, 'isSecureContext', { get: () => loc.protocol === 'https:', configurable: true });
Object.defineProperty(g, 'origin', { get: () => loc.origin, configurable: true });
Object.assign(g.navigator, {
  appVersion: '5.0 (Windows NT 10.0; Win64; x64) chrome-rs/0.1', appName: 'Netscape', appCodeName: 'Mozilla', product: 'Gecko', hardwareConcurrency: 4, maxTouchPoints: 0, deviceMemory: 8, webdriver: false, doNotTrack: null,
  plugins: [], mimeTypes: [], vibrate: () => false, javaEnabled: () => false,
  clipboard: { writeText: () => Promise.resolve(), readText: () => Promise.resolve('') },
  connection: { effectiveType: '4g', downlink: 10, rtt: 50, saveData: false, addEventListener: noop, removeEventListener: noop },
  permissions: { query: () => Promise.resolve({ state: 'denied', addEventListener: noop }) },
});
// `'onhashchange' in window` gates Backbone's iframe fallback; an <iframe> gets an empty document of its own (jQuery and Backbone only write into it)
g.onhashchange = null;
// real DOM objects have `onclick`, `oninput`... as null properties; React tests `'oninput' in document` and falls back to an IE polyfill (that crashes) without it
for (const t of 'click dblclick mousedown mouseup mousemove mouseover mouseout mouseenter mouseleave contextmenu wheel keydown keyup keypress input change beforeinput focus blur focusin focusout submit reset select scroll resize load error abort toggle copy cut paste pointerdown pointerup pointermove pointerover pointerout pointercancel dragstart dragend dragover drop animationend transitionend invalid'.split(' ')) {
  for (const o of [Element.prototype, Document.prototype]) if (!('on' + t in o)) Object.defineProperty(o, 'on' + t, { value: null, writable: true, configurable: true });
  if (!('on' + t in g)) g['on' + t] = null;
}
Object.defineProperties(Element.prototype, {
  contentDocument: { get() { if (this.localName !== 'iframe') return null; return this._cd || (this._cd = document.implementation.createHTMLDocument('')); }, configurable: true },
  contentWindow: { get() { if (this.localName !== 'iframe') return null; return this._cw || (this._cw = { document: this.contentDocument, location: { href: 'about:blank', hash: '', replace() {} }, addEventListener() {}, removeEventListener() {}, postMessage() {}, parent: g, top: g }); }, configurable: true },
});
// all zeros: pages subtract these from each other for load timings
g.performance.timing = Object.fromEntries(['navigationStart', 'fetchStart', 'requestStart', 'responseStart', 'responseEnd', 'domLoading', 'domInteractive', 'domContentLoadedEventStart', 'domContentLoadedEventEnd', 'domComplete', 'loadEventStart', 'loadEventEnd'].map(k => [k, 0]));
g.performance.getEntries = () => [];
g.performance.mark = n => ({ name: String(n), entryType: 'mark', startTime: g.performance.now(), duration: 0 });
g.performance.measure = n => ({ name: String(n), entryType: 'measure', startTime: 0, duration: 0 });

for (const k of ['addEventListener', 'removeEventListener', 'dispatchEvent']) g[k] = EventTarget.prototype[k].bind(g);

// Virtual time: runs the next timer without waiting (Rust drains promise jobs in between). `live` keeps timers beyond the first
// second queued (they run later in real time via __run_due) instead of dropping them.
const runTimer = x => {
  ran++;
  try { if (typeof x.f === 'function') x.f(...x.a); } catch (e) { __log('timer: ' + (e && (e.message + ' ' + e.stack) || e)); }
  const dead = cleared.delete(x.id);
  if (x.every && !dead && (x.live || ++x.n < 3)) { x.t = now + x.every; timers.push(x); }
};
g.__tick = live => {
  if (!timers.length) return false;
  timers.sort((a, b) => a.t - b.t || a.id - b.id);
  const x = timers[0];
  if (x.t > (live ? 1000 : 5000)) { if (!live) timers = []; return false; }
  timers.shift();
  now = Math.max(now, x.t);
  x.live = !!live;
  runTimer(x);
  return timers.length > 0;
};
// Real time: runs every timer due at clock `t` (ms); returns the delay until the next one, or -1.
g.__run_due = t => {
  if (t > now) now = t;
  for (let k = 0; k < 100; k++) {
    timers.sort((a, b) => a.t - b.t || a.id - b.id);
    const x = timers[0];
    if (!x || x.t > now) break;
    timers.shift();
    x.live = true;
    runTimer(x);
  }
  return g.__next_delay();
};
g.__next_delay = () => timers.length ? Math.max(0, Math.min(...timers.map(x => x.t)) - now) : -1;
g.__setnow = t => { if (t > now) now = t; };
g.__now = () => now;
g.__ran = () => { const r = ran; ran = 0; return r; };
g.__fire = type => {
  if (type === 'DOMContentLoaded') document.readyState = 'interactive';
  if (type === 'load') document.readyState = 'complete';
  (type === 'load' ? g : document).dispatchEvent(new Event(type, { bubbles: type !== 'load' }));
};
g.__ls_dump = () => JSON.stringify(lsData);
// composed children of a node: a shadow host shows its shadow tree, a <slot> shows the host's nodes assigned to it (or its own fallback content)
const slotted = (slot, host) => { const nm = slot._a.name || ''; return host.childNodes.filter(c => c.nodeType === 3 ? nm === '' : (c.nodeType === 1 ? (c._a.slot || '') === nm : false)); };
const composed = n => {
  const out = [];
  // constructable stylesheets adopted by a shadow root become <style> elements at its start (applied page-wide, like its own <style> children)
  if (n._sr) for (const sh of n._sr.adoptedStyleSheets || []) if (sh && sh._text) { const st = new Element('style'); st.appendChild(new Text(sh._text)); out.push(st); }
  for (const c of (n._sr || n).childNodes) {
    if (c.nodeType === 1 && c.localName === 'slot') { // a <slot> inside a shadow tree shows the nodes its host assigned to it
      let r = c; while (r.parentNode) r = r.parentNode;
      if (r.nodeType === 11 && r.host) { const a = slotted(c, r.host); out.push(...composed({ childNodes: a.length ? a : c.childNodes })); continue; }
    }
    out.push(c);
  }
  return out;
};
let ids = []; // elements/text nodes in the order of the last serialization: Rust node id N is ids[N - 1]
g.__serialize = () => {
  ids = [];
  const s = n => { ids.push(n); return n.nodeType === 3 ? n.data : n.nodeType === 8 ? { m: n.data } : { t: n.localName, a: n._a, c: composed(n).filter(c => c.nodeType === 1 || c.nodeType === 3 || c.nodeType === 8).map(s) }; };
  return JSON.stringify(s(document.documentElement));
};
const EVT = { click: 'MouseEvent', dblclick: 'MouseEvent', mousedown: 'MouseEvent', mouseup: 'MouseEvent', keydown: 'KeyboardEvent', keyup: 'KeyboardEvent', keypress: 'KeyboardEvent', input: 'InputEvent', focus: 'FocusEvent', blur: 'FocusEvent', submit: 'SubmitEvent', scroll: 'Event' };
// UI -> page: dispatch an event at a node of the last serialization; true when a handler called preventDefault()
g.__dispatch = (node, type, init) => {
  const el = node === 0 ? g : ids[node - 1];
  if (!el) return false;
  ran++;
  const o = Object.assign({ bubbles: type !== 'focus' && type !== 'blur', cancelable: true }, init ? JSON.parse(init) : {});
  const e = new (g[EVT[type]] || Event)(type, o);
  el.dispatchEvent(e);
  return e.defaultPrevented;
};
g.__setvalue = (node, v, checked) => {
  const el = ids[node - 1];
  if (!el || el.nodeType !== 1) return;
  if (checked !== null) el.checked = checked; else el.value = v;
};
g.__scrolled = y => { g.scrollY = y; g.pageYOffset = y; g.dispatchEvent(new Event('scroll')); };
g.__resize = (w, h) => { g.innerWidth = w; g.innerHeight = h; g.outerWidth = w; g.outerHeight = h; g.dispatchEvent(new Event('resize')); };
g.__boxes = list => { for (const [id, x, y, w, h] of list) { const el = ids[id - 1]; if (el && el.nodeType === 1) el._box = [x, y, w, h]; } };
// fill the intermediate prototype with the accessors (not the methods: pages polyfill those on Element.prototype and must still win)
for (const k of Reflect.ownKeys(Element.prototype)) { const d = Object.getOwnPropertyDescriptor(Element.prototype, k); if (k !== 'constructor' && d.set) Object.defineProperty(HTMLProto, k, d); }
Object.setPrototypeOf(HTMLElement.prototype, HTMLProto);
})(globalThis);
