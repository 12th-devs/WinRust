// JavaScript via QuickJS. The DOM lives in prelude.js; we ship it the parsed tree as JSON and run the page's scripts.
// Two modes: `run` (offline/tests: scripts and virtual-time timers, then read the DOM back) and `start`, a live session on
// its own thread that keeps the runtime alive after load: real-time timers, UI events, form values, layout boxes, and
// every DOM change is serialized back (throttled) so the page can be re-rendered.
// shortcut: no capture phase, no hover/mouse-move events, MutationObserver is a stub, no workers/modules; the whole DOM is
// re-serialized after each burst of activity (fine for typical pages, costly for 10k+ nodes).
use crate::{dom::{self, Dom}, load::fetch_req, render::resolve, store};
use rquickjs::{context::EvalOptions, loader::{Loader, Resolver}, module::Declared, Context, Ctx, Function, Module, Runtime, Value};
use std::{
    sync::{atomic::{AtomicU64, AtomicUsize, Ordering}, mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

const PRELUDE: &str = include_str!("prelude.js");
const IDLE: u64 = u64::MAX;

/// A page script: classic, or an ES module whose `name` (its URL) anchors relative imports.
#[derive(Clone)]
pub struct Script { pub module: bool, pub name: String, pub code: String }

impl Script {
    pub fn classic(code: impl Into<String>) -> Script { Script { module: false, name: String::new(), code: code.into() } }
    /// A `<script type="importmap">`: it is not run, it rewrites module specifiers (bare names like `react`).
    pub fn importmap(code: impl Into<String>) -> Script { Script { module: false, name: IMPORTMAP.into(), code: code.into() } }
}

/// Modules one page may import (an app bundle of a framework such as Nuxt splits into a few hundred chunks).
const MAX_MODULES: usize = 400;
const IMPORTMAP: &str = "#importmap";
type ImportMap = Arc<Mutex<Vec<(String, String)>>>;

/// Module specifiers are URLs resolved against the importing module.
struct WebResolver { map: ImportMap, page_url: String }

impl Resolver for WebResolver {
    fn resolve<'js>(&mut self, _: &Ctx<'js>, base: &str, name: &str, _: Option<rquickjs::loader::ImportAttributes<'js>>) -> rquickjs::Result<String> {
        let map = self.map.lock().map(|m| m.clone()).unwrap_or_default();
        Ok(map_specifier(&map, &self.page_url, base, name))
    }
}

/// An import map entry wins: exact name, or a `prefix/` entry (shortcut: no `scopes`, first match wins).
fn map_specifier(map: &[(String, String)], page_url: &str, base: &str, name: &str) -> String {
    for (k, v) in map {
        if k == name { return resolve(page_url, v); }
        if k.ends_with('/') && name.starts_with(k.as_str()) { return resolve(page_url, &format!("{v}{}", &name[k.len()..])); }
    }
    resolve(base, name)
}

/// Imports are fetched synchronously (same rules as fetch(): CORS for other origins, file: only from file: pages).
struct WebLoader { page_url: String, fetched: usize }

impl Loader for WebLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str, _: Option<rquickjs::loader::ImportAttributes<'js>>) -> rquickjs::Result<Module<'js, Declared>> {
        self.fetched += 1;
        let fail = || rquickjs::Error::new_loading(name);
        if self.fetched > MAX_MODULES { return Err(fail()); }
        let (status, body, _) = fetch_req(name, &self.page_url, "GET", "", "").filter(|_| name.starts_with("http") || name.starts_with("file:") && self.page_url.starts_with("file:")).ok_or_else(fail)?;
        if status >= 400 { return Err(fail()); }
        let m = Module::declare(ctx.clone(), name, body)?;
        m.meta()?.set("url", name)?; // import.meta.url (bundlers derive their public path from it)
        Ok(m)
    }
}

/// Takes the pending exception; `CHROME_RS_JSERR=1` prints it, so a page that renders blank can be diagnosed.
fn report(c: &rquickjs::Ctx) {
    let e = c.catch();
    if std::env::var_os("CHROME_RS_JSERR").is_some() {
        let m = e.as_object().map(|o| format!("{} | {}", o.get::<_, String>("message").unwrap_or_default(), o.get::<_, String>("stack").unwrap_or_default())).unwrap_or_else(|| format!("{e:?}"));
        eprintln!("JSERR {}", m.lines().take(12).collect::<Vec<_>>().join(" / "));
    }
}

fn drain(rt: &Runtime) { while rt.is_job_pending() { if rt.execute_pending_job().is_err() { break; } } }

/// A QuickJS runtime with the prelude loaded. `deadline` is the CPU-time (ms, network waits excluded) at which scripts are interrupted.
struct Vm { rt: Runtime, ctx: Context, deadline: Arc<AtomicU64>, t0: Instant, net_ms: Arc<AtomicU64>, origin: String, ls0: String, imports: ImportMap }

impl Vm {
    fn new(json: String, url: &str, max_fetches: usize, nav: Arc<dyn Fn(String) + Send + Sync>) -> Option<Vm> {
        let rt = Runtime::new().ok()?;
        rt.set_memory_limit(256 << 20);
        rt.set_max_stack_size(1 << 20);
        let deadline = Arc::new(AtomicU64::new(IDLE));
        let net_ms = Arc::new(AtomicU64::new(0)); // time spent waiting on the network does not count against the CPU budget
        let t0 = Instant::now();
        let (dl, net) = (deadline.clone(), net_ms.clone());
        rt.set_interrupt_handler(Some(Box::new(move || (t0.elapsed().as_millis() as u64).saturating_sub(net.load(Ordering::Relaxed)) > dl.load(Ordering::Relaxed))));
        let imports: ImportMap = Arc::default();
        rt.set_loader(WebResolver { map: imports.clone(), page_url: url.to_string() }, WebLoader { page_url: url.to_string(), fetched: 0 });
        let ctx = Context::full(&rt).ok()?;
        let (base, fetches) = (url.to_string(), Arc::new(AtomicUsize::new(0)));
        let origin = store::origin_of(url);
        let ls0 = store::ls_load(&origin);
        let (cookie_url, page_url, net2) = (url.to_string(), url.to_string(), net_ms.clone());
        ctx.with(|c| -> rquickjs::Result<()> {
            let g = c.globals();
            g.set("__dom_json", json)?;
            g.set("__url", url)?;
            // console.error/warn and exceptions swallowed by timers: only visible with CHROME_RS_JSERR=1
            let verbose = std::env::var_os("CHROME_RS_JSERR").is_some();
            g.set("__log", Function::new(c.clone(), move |m: String| { if verbose { eprintln!("JSLOG {}", m.chars().take(400).collect::<String>()); } })?)?;
            g.set("__ls", ls0.clone())?;
            let cu = cookie_url.clone();
            g.set("__cookie_get", Function::new(c.clone(), move || store::cookie_header(&cu, false))?)?;
            g.set("__cookie_set", Function::new(c.clone(), move |h: String| store::set_cookie(&cookie_url, &h, true))?)?;
            g.set("__parse", Function::new(c.clone(), |h: String| dom::fragment_json(&h))?)?;
            g.set("__resolve", Function::new(c.clone(), |b: String, r: String| resolve(&b, &r))?)?;
            g.set("__navigate", Function::new(c.clone(), move |u: String| nav(u))?)?;
            g.set("__fetch", Function::new(c.clone(), move |u: String, method: String, body: String, ctype: String| -> String {
                let full = resolve(&base, &u);
                if !(full.starts_with("http") || full.starts_with("file:") && page_url.starts_with("file:")) || fetches.fetch_add(1, Ordering::Relaxed) >= max_fetches { return r#"{"status":0,"body":""}"#.into(); }
                let t = Instant::now();
                let r = fetch_req(&full, &page_url, &method, &body, &ctype);
                net2.fetch_add(t.elapsed().as_millis() as u64, Ordering::Relaxed);
                let (status, body, ctype) = r.unwrap_or((0, String::new(), String::new()));
                serde_json::json!({ "status": status, "body": body, "ctype": ctype }).to_string()
            })?)?;
            // a prelude that fails to load switches all page JavaScript off: say why when CHROME_RS_JSERR is set
            if let Err(e) = c.eval::<Value, _>(PRELUDE) { report(&c); return Err(e); }
            c.eval::<Value, _>("var __cs = null;")?;
            // corpus replays must be repeatable: shuffled sponsor lists, random ids and the like come out the same every time
            if crate::load::replaying() { c.eval::<Value, _>("Math.random = (() => { let s = 12345; return () => (s = (s * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff; })();")?; }
            Ok(())
        }).ok()?;
        Some(Vm { rt, ctx, deadline, t0, net_ms, origin, ls0, imports })
    }

    fn cpu(&self) -> u64 { (self.t0.elapsed().as_millis() as u64).saturating_sub(self.net_ms.load(Ordering::Relaxed)) }
    fn arm(&self, ms: u64) { self.deadline.store(self.cpu() + ms, Ordering::Relaxed); }
    fn disarm(&self) { self.deadline.store(IDLE, Ordering::Relaxed); }
    fn eval(&self, src: &str) { self.ctx.with(|c| if c.eval::<Value, _>(src).is_err() { report(&c); }); drain(&self.rt); }
    fn run_script(&self, s: &Script) {
        if s.name == IMPORTMAP {
            if let (Ok(v), Ok(mut m)) = (serde_json::from_str::<serde_json::Value>(&s.code), self.imports.lock()) {
                m.extend(v["imports"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))));
            }
            return;
        }
        if !s.module {
            // document.currentScript: a <script> element carrying the src, which bundlers (Turbopack, webpack) read to find their chunks
            let name = serde_json::to_string(&s.name).unwrap_or_default();
            self.eval(&format!("__cs = {name};"));
            // page scripts are sloppy-mode JavaScript: rquickjs evals in strict mode by default, where `OSM = {}` without `var` throws
            self.ctx.with(|c| { let mut o = EvalOptions::default(); o.strict = false; if c.eval_with_options::<Value, _>(s.code.as_str(), o).is_err() { report(&c); } });
            drain(&self.rt);
            return self.eval("__cs = null;");
        }
        self.ctx.with(|c| {
            let run = || -> rquickjs::Result<()> {
                let m = Module::declare(c.clone(), s.name.as_str(), s.code.as_str())?;
                m.meta()?.set("url", s.name.as_str())?;
                let (_, p) = m.eval()?;
                p.finish::<Value>().map(|_| ())
            };
            if run().is_err() { report(&c); }
        });
        drain(&self.rt);
    }
    fn eval_bool(&self, src: &str) -> bool { let r = self.ctx.with(|c| c.eval::<bool, _>(src).unwrap_or_else(|_| { let _ = c.catch(); false })); drain(&self.rt); r }
    fn eval_num(&self, src: &str) -> f64 { let r = self.ctx.with(|c| c.eval::<f64, _>(src).unwrap_or_else(|_| { let _ = c.catch(); -1.0 })); drain(&self.rt); r }
    fn serialize(&self) -> Option<String> { self.ctx.with(|c| c.eval::<String, _>("__serialize()").ok()) }

    /// Persists localStorage when the page changed it.
    fn save_ls(&mut self) {
        let ls1 = self.ctx.with(|c| c.eval::<String, _>("__ls_dump()").ok());
        if let Some(ls1) = ls1.filter(|l| *l != self.ls0 && self.origin.starts_with("http")) { store::ls_save(&self.origin, &ls1); self.ls0 = ls1; }
    }

    /// Page scripts, DOMContentLoaded, load, then timers in virtual time (`live`: only the first second, the rest stays queued).
    fn initial_phase(&self, scripts: &[Script], live: bool) {
        self.arm(4000);
        for s in scripts { self.run_script(s); }
        self.eval("__fire('DOMContentLoaded')");
        self.eval("__fire('load')");
        let tick = if live { "__tick(true)" } else { "__tick()" };
        for _ in 0..300 { if !self.eval_bool(tick) { break; } }
    }
}

fn exec(json: String, scripts: &[Script], url: &str) -> Option<String> {
    let mut vm = Vm::new(json, url, 12, Arc::new(|_| {}))?;
    vm.initial_phase(scripts, false);
    vm.disarm();
    vm.save_ls();
    vm.serialize()
}

/// Runs `scripts` (document order) against `dom`; on any failure the DOM is left untouched.
pub fn run(dom: &mut Dom, scripts: &[Script], url: &str) {
    let Some(&html) = dom.nodes[0].kids.first() else { return };
    let Some(out) = exec(dom.to_json(html).to_string(), scripts, url) else { return };
    if let Some(d) = serde_json::from_str(&out).ok().and_then(|v| Dom::from_json(&v)) { *dom = d; }
}

/// What the UI can ask the live page to do. `node` is a Dom node id of the last serialization the UI saw.
enum Cmd {
    Event { node: usize, kind: String, init: String, reply: Option<mpsc::Sender<bool>> },
    Value { node: usize, value: String, checked: Option<bool> },
    Resize { w: u32, h: u32 },
    Scroll(f32),
    Boxes(String),
}

/// Sender side of a live session; cloning is cheap, dropping the last clone ends the session thread.
#[derive(Clone)]
pub struct Handle { tx: mpsc::Sender<Cmd> }

impl Handle {
    /// Dispatches a bubbling event and waits (at most 400 ms) to learn whether a handler called preventDefault().
    pub fn event(&self, node: usize, kind: &str, init: &str) -> bool {
        let (rtx, rrx) = mpsc::channel();
        self.tx.send(Cmd::Event { node, kind: kind.into(), init: init.into(), reply: Some(rtx) }).is_ok() && rrx.recv_timeout(Duration::from_millis(400)).unwrap_or(false)
    }
    /// Fire-and-forget event.
    pub fn post(&self, node: usize, kind: &str, init: &str) { let _ = self.tx.send(Cmd::Event { node, kind: kind.into(), init: init.into(), reply: None }); }
    /// Mirrors what the user typed/checked into the page's DOM (no event is fired).
    pub fn value(&self, node: usize, value: &str, checked: Option<bool>) { let _ = self.tx.send(Cmd::Value { node, value: value.into(), checked }); }
    pub fn resize(&self, w: u32, h: u32) { let _ = self.tx.send(Cmd::Resize { w, h }); }
    /// The user scrolled the page to `y` CSS px: updates scrollY and fires `scroll`.
    pub fn scroll(&self, y: f32) { let _ = self.tx.send(Cmd::Scroll(y)); }
    /// Layout boxes `[[node, x, y, w, h], ...]` for getBoundingClientRect() and friends.
    pub fn boxes(&self, json: String) { let _ = self.tx.send(Cmd::Boxes(json)); }
}

/// Starts a live session. `on_update` receives every changed DOM serialization (JSON, same shape as `Dom::to_json`).
/// Returns the handle and the serialization after the load phase; None when the runtime could not start.
pub fn start(dom_json: String, scripts: Vec<Script>, url: String, on_update: Box<dyn Fn(String) + Send>, nav: Arc<dyn Fn(String) + Send + Sync>) -> Option<(Handle, String)> {
    let (tx, rx) = mpsc::channel::<Cmd>();
    let (itx, irx) = mpsc::channel::<Option<String>>();
    std::thread::Builder::new().stack_size(16 << 20).spawn(move || live_main(dom_json, scripts, url, rx, itx, on_update, nav)).ok()?;
    let first = irx.recv_timeout(Duration::from_secs(40)).ok()??;
    Some((Handle { tx }, first))
}

fn live_main(dom_json: String, scripts: Vec<Script>, url: String, rx: mpsc::Receiver<Cmd>, itx: mpsc::Sender<Option<String>>, on_update: Box<dyn Fn(String) + Send>, nav: Arc<dyn Fn(String) + Send + Sync>) {
    let Some(mut vm) = Vm::new(dom_json, &url, 400, nav) else { let _ = itx.send(None); return };
    vm.initial_phase(&scripts, true);
    vm.disarm();
    vm.save_ls();
    let Some(first) = vm.serialize() else { let _ = itx.send(None); return };
    let _ = itx.send(Some(first.clone()));
    let (base, live_start) = (vm.eval_num("__now()"), Instant::now());
    let clock = || base + live_start.elapsed().as_millis() as f64;
    let (mut last, mut last_sent, mut dirty) = (first, Instant::now(), false);
    let mut next = vm.eval_num("__next_delay()");
    const THROTTLE: Duration = Duration::from_millis(60);
    loop {
        let mut wait: Option<Duration> = if next >= 0.0 { Some(Duration::from_millis(next.max(1.0) as u64)) } else { None };
        if dirty { let left = THROTTLE.saturating_sub(last_sent.elapsed()).max(Duration::from_millis(1)); wait = Some(wait.map_or(left, |w| w.min(left))); }
        let cmd = match wait {
            None => match rx.recv() { Ok(c) => Some(c), Err(_) => break },
            Some(w) => match rx.recv_timeout(w) { Ok(c) => Some(c), Err(mpsc::RecvTimeoutError::Timeout) => None, Err(_) => break },
        };
        vm.arm(2000); // one burst of script work may use two seconds of CPU
        vm.eval(&format!("__setnow({})", clock()));
        if let Some(c) = cmd {
            dirty = true;
            match c {
                Cmd::Event { node, kind, init, reply } => {
                    let prevented = vm.eval_bool(&format!("__dispatch({node},{},{})", serde_json::json!(kind), serde_json::json!(init)));
                    if let Some(r) = reply { let _ = r.send(prevented); }
                }
                Cmd::Value { node, value, checked } => vm.eval(&format!("__setvalue({node},{},{})", serde_json::json!(value), checked.map_or("null".to_string(), |c| c.to_string()))),
                Cmd::Resize { w, h } => vm.eval(&format!("__resize({w},{h})")),
                Cmd::Scroll(y) => vm.eval(&format!("__scrolled({y})")),
                Cmd::Boxes(json) => vm.eval(&format!("__boxes({json})")),
            }
        }
        next = vm.eval_num(&format!("__run_due({})", clock()));
        if vm.eval_num("__ran()") > 0.0 { dirty = true; }
        vm.disarm();
        if dirty && last_sent.elapsed() >= THROTTLE {
            dirty = false;
            last_sent = Instant::now();
            vm.save_ls();
            if let Some(s) = vm.serialize() { if s != last { on_update(s.clone()); last = s; } }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn import_map_rewrites_bare_specifiers() {
        let map = vec![("react".to_string(), "https://cdn.test/react-1.js".to_string()), ("lib/".to_string(), "/vendor/lib/".to_string())];
        assert_eq!(map_specifier(&map, "https://a.com/p", "https://a.com/x/m.js", "react"), "https://cdn.test/react-1.js");
        assert_eq!(map_specifier(&map, "https://a.com/p", "https://a.com/x/m.js", "lib/dom.js"), "https://a.com/vendor/lib/dom.js");
        assert_eq!(map_specifier(&map, "https://a.com/p", "https://a.com/x/m.js", "./other.js"), "https://a.com/x/other.js");
    }

    use super::*;

    fn run_html(html: &str, js: &[&str]) -> Dom {
        let mut d = dom::parse(html, true);
        run(&mut d, &js.iter().map(|s| Script::classic(*s)).collect::<Vec<_>>(), "https://a.com/p?q=1");
        d
    }

    #[test]
    fn custom_elements_links_and_forms() {
        let d = run_html("<my-box id=pre label=L1>x</my-box><a id=l href='/d/x.html?q=1#top'>l</a><form id=f><input name=a><select name=s><option>o</option></select></form><p id=o></p>", &["
            var log = [];
            class MyBox extends HTMLElement {
              static get observedAttributes() { return ['label']; }
              constructor() { super(); log.push('ctor'); }
              connectedCallback() { log.push('conn ' + this.id); }
              attributeChangedCallback(n, o, v) { log.push(n + ':' + o + '>' + v); }
            }
            customElements.define('my-box', MyBox);
            var e = document.createElement('my-box'); e.id = 'new'; document.body.appendChild(e);
            var a = document.getElementById('l'), f = document.getElementById('f');
            log.push(a.href, a.pathname, a.search, a.hash, f.elements.length, f.elements.s.options.length, document.getElementById('pre') instanceof MyBox);
            document.getElementById('o').textContent = log.join('|');
        "]);
        assert_eq!(d.text_content(d.find("p").unwrap()), "ctor|label:null>L1|conn pre|ctor|conn new|https://a.com/d/x.html?q=1#top|/d/x.html|?q=1|#top|2|1|true");
    }

    #[test]
    fn get_computed_style_runs_a_small_cascade() {
        let d = run_html("<style>.hid{display:none} #m{display:block !important} div p{color:blue} .box{width:50%}</style><div class=wrap style='visibility:hidden'><p id=p class=hid>x</p><span id=s>y</span></div><main id=m class=hid></main><section id=sec></section><output id=o></output>", &[
            "var c = id => getComputedStyle(document.getElementById(id)); var r = [c('p').display, c('m').display, c('sec').display, c('s').display, c('p').color, c('s').visibility, c('p').getPropertyValue('display'), c('o').opacity, document.createElement('i') && getComputedStyle(document.body).display];
             document.getElementById('o').textContent = r.join('|');",
        ]);
        assert_eq!(d.text_content(d.find("output").unwrap()), "none|block|block|inline|blue|hidden|none|1|block");
    }

    #[test]
    fn scripts_mutate_dom() {
        let d = run_html("<p id=a>old</p><ul id=l></ul>", &[
            "document.getElementById('a').textContent = 'new ' + location.hostname;",
            "var l = document.querySelector('#l'); for (var i=0;i<2;i++){ var li=document.createElement('li'); li.className='i'+i; li.textContent=i; l.appendChild(li); }",
            "document.title='T'; document.body.insertAdjacentHTML('beforeend','<b class=x>bold</b>');",
            "document.querySelector('.i1').style.color='red';",
        ]);
        assert_eq!(d.text_content(d.find("p").unwrap()), "new a.com");
        assert_eq!(d.nodes.iter().filter(|n| n.tag == "li").count(), 2);
        assert_eq!(d.text_content(d.find("title").unwrap()), "T");
        assert_eq!(d.text_content(d.find("b").unwrap()), "bold");
        let li = d.descendants().into_iter().filter(|&i| d.nodes[i].tag == "li").nth(1).unwrap();
        assert_eq!(d.attr(li, "style"), Some("color:red"));
    }

    #[test]
    fn events_timers_promises_and_errors() {
        let d = run_html("<div id=o></div>", &[
            "throw new Error('boom');",
            "var log=[]; document.addEventListener('DOMContentLoaded',()=>log.push('dcl')); window.addEventListener('load',()=>log.push('load'));",
            "setTimeout(()=>log.push('t2'),20); setTimeout(()=>log.push('t1'),10); Promise.resolve().then(()=>log.push('p'));",
            "window.addEventListener('load',()=>setTimeout(()=>{document.getElementById('o').textContent=log.join(',')},100));",
        ]);
        assert_eq!(d.text_content(d.find("div").unwrap()), "p,dcl,load,t1,t2");
    }

    #[test]
    fn document_cookie_reads_and_writes_the_jar() {
        let d = run_html("<p id=o></p>", &["document.cookie='zz=1; path=/'; document.getElementById('o').textContent = document.cookie;"]);
        assert_eq!(d.text_content(d.find("p").unwrap()), "zz=1");
        assert!(store::cookie_header("https://a.com/p", false).contains("zz=1"));
    }

    #[test]
    fn web_apis_url_encoding_forms_history() {
        let d = run_html("<form id=f><input name=a value='x y'><input type=checkbox name=c checked value=on><input type=checkbox name=n><select name=s><option value=1>One<option value=2 selected>Two</select><textarea name=t>hi</textarea><input type=submit name=go value=Go></form><div id=o></div>", &[r#"
            const out = [];
            const u = new URL('../b/c?x=1&y=a%20b#h', 'https://u:p@Example.com:8080/a/d/e');
            out.push(u.href, u.origin, u.searchParams.get('y'));
            u.searchParams.append('z', 'q r'); out.push(u.search);
            out.push(new URLSearchParams({ k: 'v w', e: '' }).toString(), URL.canParse('nope'));
            out.push(btoa('hello'), atob('aGVsbG8='), new TextDecoder().decode(new TextEncoder().encode('héllo €')));
            out.push(new URLSearchParams(new FormData(document.getElementById('f'))).toString());
            history.pushState({ n: 1 }, '', '/next?q=2'); out.push(location.pathname + location.search, JSON.stringify(history.state), history.length);
            out.push(typeof performance.now(), typeof queueMicrotask, crypto.randomUUID().length, structuredClone({ a: [1] }).a[0]);
            queueMicrotask(() => out.push('micro'));
            fetch('ftp://nope').then(() => out.push('ok'), () => out.push('rejected')).then(() => { document.getElementById('o').textContent = out.join('|'); });
        "#]);
        assert_eq!(d.text_content(d.find("div").unwrap()),
            "https://u:p@example.com:8080/a/b/c?x=1&y=a%20b#h|https://example.com:8080|a b|?x=1&y=a+b&z=q+r|k=v+w&e=|false|aGVsbG8=|hello|héllo €|a=x+y&c=on&s=2&t=hi|/next?q=2|{\"n\":1}|2|number|function|36|1|micro|rejected");
    }

    #[test]
    fn infinite_loop_is_cut_off_and_dom_survives() {
        let t = Instant::now();
        let d = run_html("<p>keep</p>", &["while(true){}"]);
        assert!(t.elapsed() < Duration::from_secs(8));
        assert_eq!(d.text_content(d.find("p").unwrap()), "keep");
    }

    fn wait_for(rx: &mpsc::Receiver<String>, pred: impl Fn(&Dom) -> bool, ms: u64) -> Option<Dom> {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            if let Ok(s) = rx.recv_timeout(Duration::from_millis(50)) {
                if let Some(d) = serde_json::from_str(&s).ok().and_then(|v| Dom::from_json(&v)) { if pred(&d) { return Some(d); } }
            }
        }
        None
    }

    #[test]
    fn live_session_runs_real_time_timers_events_and_values() {
        let d = dom::parse("<p id=c>0</p><button id=b>go</button><input id=i name=q value=a><p id=o></p>", true);
        let html = d.nodes[0].kids[0];
        let script = r#"
            var n = 0, el = id => document.getElementById(id);
            setInterval(() => { n++; el('c').textContent = String(n); }, 40);
            setTimeout(() => { el('o').textContent = 'late'; }, 1500);
            el('b').addEventListener('click', e => { e.preventDefault(); el('b').textContent = 'clicked:' + e.type; });
            el('i').addEventListener('input', e => { el('o').textContent = 'typed:' + e.target.value; });
            window.addEventListener('resize', () => { el('o').textContent = 'resized:' + innerWidth; });
        "#;
        let (utx, urx) = mpsc::channel();
        let (h, first) = start(d.to_json(html).to_string(), vec![Script::classic(script)], "https://a.com/".into(), Box::new(move |s| { let _ = utx.send(s); }), Arc::new(|_| {})).unwrap();
        let d0 = Dom::from_json(&serde_json::from_str(&first).unwrap()).unwrap();
        let n0: u32 = d0.text_content(d0.find("p").unwrap()).parse().unwrap();
        assert!((20..=26).contains(&n0), "first second of the interval ran in virtual time: {n0}");
        // the interval keeps running in real time after load
        let d1 = wait_for(&urx, |d| d.text_content(d.find("p").unwrap()).parse::<u32>().is_ok_and(|n| n > n0 + 3), 3000).expect("interval kept ticking");
        let button = d1.find("button").unwrap();
        assert!(h.event(button, "click", "{}"), "preventDefault is reported back");
        wait_for(&urx, |d| d.text_content(d.find("button").unwrap()) == "clicked:click", 3000).expect("click handler ran");
        // typed text is mirrored into the DOM, then the input event sees it
        let input = d1.find("input").unwrap();
        h.value(input, "hello", None);
        h.post(input, "input", "{}");
        wait_for(&urx, |d| d.text_content(d.nodes[d.find("p").unwrap()].kids.len().checked_sub(0).map_or(0, |_| d.descendants().into_iter().filter(|&i| d.nodes[i].tag == "p").nth(1).unwrap())) == "typed:hello", 3000).expect("input event saw the value");
        h.resize(640, 480);
        wait_for(&urx, |d| d.text_content(d.descendants().into_iter().filter(|&i| d.nodes[i].tag == "p").nth(1).unwrap()) == "resized:640", 3000).expect("resize event");
    }

    #[test]
    fn layout_boxes_feed_getboundingclientrect() {
        let d = dom::parse("<div id=a>x</div><p id=o></p>", true);
        let html = d.nodes[0].kids[0];
        let (utx, urx) = mpsc::channel();
        let (h, first) = start(d.to_json(html).to_string(), vec![Script::classic("window.addEventListener('click', () => { var r = document.getElementById('a').getBoundingClientRect(); document.getElementById('o').textContent = [r.left, r.top, r.width, r.height, document.getElementById('a').offsetHeight].join(','); });")], "https://a.com/".into(), Box::new(move |s| { let _ = utx.send(s); }), Arc::new(|_| {})).unwrap();
        // node ids are those of the serialization the UI saw (pre-order), not of the parsed DOM
        let d0 = Dom::from_json(&serde_json::from_str(&first).unwrap()).unwrap();
        let a = d0.descendants().into_iter().find(|&i| d0.attr(i, "id") == Some("a")).unwrap();
        h.boxes(format!("[[{a}, 10, 20, 300.5, 40]]"));
        h.post(a, "click", "{}");
        wait_for(&urx, |d| d.text_content(d.find("p").unwrap()) == "10,20,300.5,40,40", 3000).expect("boxes visible to scripts");
    }
}

