// Browser shell: window, tabs, painting, input. Page loading is in load.rs, layout in layout.rs, live page scripts in js.rs.
// shortcut: one softbuffer surface painted on the CPU every frame; the UI thread lays out the page itself.
mod browse;
mod css;
mod dom;
mod gfx;
mod js;
mod layout;
mod load;
mod render;
mod snap;
mod store;
mod svg;

use gfx::{draw_field, draw_image, draw_popup, draw_text, fill, Fonts, POPUP_ROWS};
use browse::SelPoint;
use layout::{Placed, Rect};
use render::{Field, FieldKind, Page};
use std::{num::NonZeroU32, sync::Arc};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalPosition,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{CursorIcon, Window, WindowId},
};

const TAB_H: f32 = 32.0;
const BAR: f32 = 40.0;
const HOME: &str = "https://example.com";
enum Msg {
    Loaded { uid: u64, id: u64, url: String, push: bool, res: Result<(String, Page), String> },
    /// A live page re-rendered after script activity (or a script asked to navigate); `id` is the navigation that owns the session.
    Live { uid: u64, id: u64, ev: load::LiveEvent },
}

// ---------- text editing helpers (pure, unit-tested) ----------

fn byte_at(s: &str, ci: usize) -> usize { s.char_indices().nth(ci).map_or(s.len(), |(b, _)| b) }

/// Char range [start, end) of the logical (newline-delimited) line holding `caret`.
fn line_bounds(s: &str, caret: usize) -> (usize, usize) {
    let chars: Vec<char> = s.chars().collect();
    let c = caret.min(chars.len());
    let start = chars[..c].iter().rposition(|&ch| ch == '\n').map_or(0, |p| p + 1);
    let end = chars[c..].iter().position(|&ch| ch == '\n').map_or(chars.len(), |p| c + p);
    (start, end)
}

/// Applies an editing key to a text-like field. Returns true when the key was consumed.
fn edit_field(fl: &mut Field, caret: &mut usize, key: &Key, text: Option<&str>) -> bool {
    let len = fl.value.chars().count();
    *caret = (*caret).min(len);
    match key {
        Key::Named(NamedKey::Backspace) => if *caret > 0 { *caret -= 1; fl.value.remove(byte_at(&fl.value, *caret)); },
        Key::Named(NamedKey::Delete) => if *caret < len { fl.value.remove(byte_at(&fl.value, *caret)); },
        Key::Named(NamedKey::ArrowLeft) => *caret = caret.saturating_sub(1),
        Key::Named(NamedKey::ArrowRight) => *caret = (*caret + 1).min(len),
        Key::Named(NamedKey::Home) => *caret = line_bounds(&fl.value, *caret).0,
        Key::Named(NamedKey::End) => *caret = line_bounds(&fl.value, *caret).1,
        Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown) if fl.kind == FieldKind::TextArea => {
            let (s, e) = line_bounds(&fl.value, *caret);
            let col = *caret - s;
            let up = matches!(key, Key::Named(NamedKey::ArrowUp));
            if up && s > 0 { let (ps, pe) = line_bounds(&fl.value, s - 1); *caret = (ps + col).min(pe); }
            else if !up && e < len { let (ns, ne) = line_bounds(&fl.value, e + 1); *caret = (ns + col).min(ne); }
        }
        Key::Named(NamedKey::Enter) if fl.kind == FieldKind::TextArea => { fl.value.insert(byte_at(&fl.value, *caret), '\n'); *caret += 1; }
        _ => match text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
            Some(t) if !matches!(key, Key::Named(NamedKey::Enter | NamedKey::Escape | NamedKey::Tab)) => {
                fl.value.insert_str(byte_at(&fl.value, *caret), t);
                *caret += t.chars().count();
            }
            _ => return false,
        },
    }
    true
}

// ---------- URLs and forms ----------

/// (url, post body) for submitting `form` with the given name/value pairs.
fn form_request(form: &render::Form, pairs: &[(&str, &str)]) -> (String, Option<String>) {
    let query = pairs.iter().map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v))).collect::<Vec<_>>().join("&");
    if form.post { (form.action.clone(), Some(query)) }
    else { (format!("{}?{query}", form.action.split(['?', '#']).next().unwrap_or("")), None) }
}

fn percent_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Address-bar text -> URL: full URL as is, bare host gets https://, anything else is a search.
/// Address-bar text -> URL; `engine` is a search URL with `{q}` for the query.
fn to_url_with(input: &str, engine: &str) -> String {
    let s = input.trim();
    if s.contains("://") || ["data:", "about:", "view-source:"].iter().any(|p| s.starts_with(p)) { s.into() }
    else if !s.contains(' ') && (s.contains('.') || s.starts_with("localhost")) { format!("https://{s}") }
    else { engine.replace("{q}", &percent_encode(s)) }
}

const ENGINE: &str = "https://html.duckduckgo.com/html/?q={q}";

fn to_url(input: &str) -> String { to_url_with(input, &store::setting("search").filter(|e| e.contains("{q}")).unwrap_or_else(|| ENGINE.into())) }

const ZOOMS: &[f32] = &[0.3, 0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];

/// Next preset zoom level above/below `z`.
fn zoom_step(z: f32, up: bool) -> f32 {
    if up { ZOOMS.iter().copied().find(|&v| v > z + 0.001).unwrap_or(5.0) } else { ZOOMS.iter().rev().copied().find(|&v| v < z - 0.001).unwrap_or(0.3) }
}

/// Index of `fields[i]` among the fields with the same kind and name (so fields can be matched across rebuilt pages).
fn occurrence(fields: &[Field], i: usize) -> usize { fields[..i].iter().filter(|f| f.kind == fields[i].kind && f.name == fields[i].name).count() }

fn find_occurrence(fields: &[Field], kind: FieldKind, name: &str, n: usize) -> Option<usize> {
    fields.iter().enumerate().filter(|(_, f)| f.kind == kind && f.name == name).nth(n).map(|(i, _)| i)
}

/// Carries the user's edits over to a freshly built live page: a field keeps what the user typed unless a script changed its value.
fn merge_fields(old: &[Field], new: &mut [Field]) {
    for k in 0..new.len() {
        let n = occurrence(new, k);
        let Some(o) = find_occurrence(old, new[k].kind, &new[k].name, n).map(|i| &old[i]) else { continue };
        let nf = &mut new[k];
        if nf.value == o.initial { nf.value = o.value.clone(); nf.sel = o.sel; }
        if nf.checked == o.initial_checked { nf.checked = o.checked; }
    }
}

/// DOM `key` value for the page's keydown/keyup events.
fn key_name(k: &Key) -> Option<String> {
    Some(match k {
        Key::Character(c) => c.to_string(),
        Key::Named(n) => match n {
            NamedKey::Enter => "Enter", NamedKey::Escape => "Escape", NamedKey::Backspace => "Backspace", NamedKey::Tab => "Tab", NamedKey::Delete => "Delete",
            NamedKey::ArrowUp => "ArrowUp", NamedKey::ArrowDown => "ArrowDown", NamedKey::ArrowLeft => "ArrowLeft", NamedKey::ArrowRight => "ArrowRight",
            NamedKey::Home => "Home", NamedKey::End => "End", NamedKey::PageUp => "PageUp", NamedKey::PageDown => "PageDown", NamedKey::Space => " ",
            _ => return None,
        }.to_string(),
        _ => return None,
    })
}

/// State of the find bar.
struct Find { query: String, matches: Vec<browse::Match>, cur: usize }

enum Drag { None, Select { anchor: SelPoint, moved: bool, down: (f32, f32) }, Scrollbar { grab: f32 } }

// ---------- tabs ----------

struct Tab {
    uid: u64,
    page: Page,
    fields: Vec<Field>, // live copy of page.fields (what the user typed/selected)
    focus: Option<usize>, // focused page field; None = address bar
    caret: usize, // char index inside the focused field
    popup: Option<usize>, // open select list
    placed: Vec<Placed>,
    rects: Vec<Rect>,
    content_h: f32,
    layout_w: u32,
    layout_sc: f32,
    layout_vh: u32, // viewport height the layout was made for (position: fixed offsets depend on it)
    hover: usize,   // DOM node under the pointer (0 = none), for :hover
    active: usize,  // DOM node of the pressed mouse button, for :active
    applied: (usize, usize, usize), // (hover, active, focus) the item stream was last styled with
    pinned: Vec<(bool, usize, f32, f32, f32)>, // fixed/sticky items: (is rect, index, laid-out y, scroll offset at which it pins, travel limit or NAN)
    groups: std::collections::HashMap<u32, layout::Group>, // scroll containers of the layout
    moved: Vec<(bool, usize, f32, u32)>, // items inside scroll containers: (is rect, index, laid-out y, group)
    box_scroll: std::collections::HashMap<usize, f32>, // scroll offset per container node (device px)
    box_applied: bool, // `moved` items are placed for the current `box_scroll`
    pinned_at: f32, // scroll offset the pinned items are currently placed for (NAN = must be updated)
    dirty: bool,
    scroll: f32,
    url: String,
    title: String,
    input: String,
    fresh: bool, // address bar text is "selected": next key replaces it
    loading: bool,
    nav_id: u64,
    hist: Vec<String>,
    hpos: usize,
    zoom: f32,
    target: Option<f32>, // smooth-scroll destination
    sel: Option<(SelPoint, SelPoint)>,
    sugg: Vec<store::Entry>,
    sugg_sel: Option<usize>,
    bookmarked: bool,
    live: Option<js::Handle>, // running page scripts (events in, DOM updates out)
    scroll_sent: f32,
    scroll_at: std::time::Instant,
}

impl Tab {
    fn new(uid: u64) -> Tab {
        Tab {
            uid, page: Page { bg: 0xFFFFFF, ..Page::default() }, fields: vec![], focus: None, caret: 0, popup: None, placed: vec![], rects: vec![],
            content_h: 0.0, layout_w: 0, layout_sc: 1.0, layout_vh: 0, hover: 0, active: 0, applied: (0, 0, 0), pinned: vec![], pinned_at: f32::NAN, groups: Default::default(), moved: vec![], box_scroll: Default::default(), box_applied: false, dirty: true, scroll: 0.0, url: String::new(), title: String::new(), input: String::new(),
            fresh: false, loading: false, nav_id: 0, hist: vec![], hpos: 0, zoom: 1.0, target: None, sel: None, sugg: vec![], sugg_sel: None, bookmarked: false, live: None, scroll_sent: 0.0, scroll_at: std::time::Instant::now(),
        }
    }

    fn blank(uid: u64) -> Tab {
        let mut t = Tab::new(uid);
        t.page = render::build(&dom::parse("<h2>New tab</h2><p>Type an address or search words above and press Enter.</p>", true), "about:blank", Default::default());
        t.title = "New tab".into();
        t.fresh = true;
        t
    }

    fn navigate(&mut self, proxy: &EventLoopProxy<Msg>, url: String, push: bool, post: Option<String>) {
        self.nav_id += 1;
        self.loading = true;
        let (uid, id, proxy) = (self.uid, self.nav_id, proxy.clone());
        let live_proxy = proxy.clone();
        let hook: load::LiveHook = Box::new(move |ev| { let _ = live_proxy.send_event(Msg::Live { uid, id, ev }); });
        // Big stack: DOM/JSON/style recursion is bounded by dom::MAX_DEPTH but frames are large in debug builds.
        let _ = std::thread::Builder::new().stack_size(32 << 20).spawn(move || {
            let res = load::load(&url, post.as_deref(), Some(hook));
            let _ = proxy.send_event(Msg::Loaded { uid, id, url, push, res });
        });
    }

    fn loaded(&mut self, id: u64, url: String, push: bool, res: Result<(String, Page), String>) {
        if id != self.nav_id { return; } // a newer navigation superseded this one
        self.loading = false;
        let url = match res {
            Ok((final_url, mut page)) => {
                self.live = page.live.take();
                self.page = page;
                if push {
                    self.hist.truncate(if self.hist.is_empty() { 0 } else { self.hpos + 1 });
                    self.hist.push(final_url.clone());
                    self.hpos = self.hist.len() - 1;
                }
                final_url
            }
            Err(e) => {
                self.live = None;
                let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
                self.page = render::build(&dom::parse(&format!("<h2>Could not load page</h2><p>{}</p><p>{}</p>", esc(&url), esc(&e)), true), &url, Default::default());
                url
            }
        };
        self.title = if self.page.title.is_empty() { url.clone() } else { self.page.title.clone() };
        self.input = url.clone();
        self.url = url;
        self.fresh = false;
        self.fields = self.page.fields.clone();
        self.focus = None;
        self.popup = None;
        self.scroll = 0.0;
        self.target = None;
        self.sel = None;
        self.sugg.clear();
        self.bookmarked = store::is_bookmarked(&self.url);
        store::add_history(&self.url, &self.title);
        self.dirty = true;
    }

    /// A live page re-rendered itself: swap in the new page, keep scroll, zoom and what the user typed.
    fn live_update(&mut self, id: u64, mut page: Page) {
        if id != self.nav_id || self.loading && self.live.is_none() { return; }
        merge_fields(&self.fields, &mut page.fields);
        self.focus = self.focus.and_then(|f| find_occurrence(&page.fields, self.fields[f].kind, &self.fields[f].name.clone(), occurrence(&self.fields, f)));
        self.popup = self.popup.filter(|&p| Some(p) == self.focus);
        self.fields = page.fields.clone();
        if !page.title.is_empty() { self.title = page.title.clone(); }
        self.page = page;
        self.sel = None;
        self.dirty = true;
    }

    /// Mirrors field `i` (typed text, check state, chosen option) into the page's DOM and fires `input`.
    fn live_value(&self, i: usize, fire: &[&str]) {
        let (Some(h), Some(f)) = (&self.live, self.fields.get(i)) else { return };
        match f.kind {
            FieldKind::Check | FieldKind::Radio => h.value(f.node, "", Some(f.checked)),
            _ => h.value(f.node, &f.value, None),
        }
        for k in fire { h.post(f.node, k, "{}"); }
    }

    /// Focus moves between page fields: blur/change for the old one, focus for the new one.
    fn set_focus(&mut self, new: Option<usize>) {
        if self.focus == new { return; }
        if let Some(i) = self.focus { self.commit_change(i); }
        if let Some(h) = &self.live {
            if let Some(f) = self.focus.and_then(|i| self.fields.get(i)) { h.post(f.node, "blur", "{}"); }
            if let Some(f) = new.and_then(|i| self.fields.get(i)) { h.post(f.node, "focus", "{}"); }
        }
        self.focus = new;
    }

    /// Node that receives key events: the focused field, else the page body.
    fn key_target(&self) -> usize { self.focus.and_then(|i| self.fields.get(i)).map_or(self.page.body, |f| f.node) }

    fn refresh_sugg(&mut self) {
        self.sugg = if self.focus.is_none() && !self.fresh { store::suggest(&self.input, 6) } else { vec![] };
        self.sugg_sel = None;
    }

    fn history(&mut self, proxy: &EventLoopProxy<Msg>, back: bool) {
        let target = if back { self.hpos.checked_sub(1) } else { Some(self.hpos + 1).filter(|&p| p < self.hist.len()) };
        if let Some(p) = target {
            self.hpos = p;
            let u = self.hist[p].clone();
            self.navigate(proxy, u, false, None);
        }
    }

    /// Submits the form that owns `field`; `submitter` (a button) contributes its own name/value.
    /// Enter or blur commits a changed field: `change` fires once per edit (a script that clears the field afterwards must not be overwritten by the old text on re-render).
    fn commit_change(&mut self, i: usize) {
        let Some(f) = self.fields.get_mut(i) else { return };
        if !matches!(f.kind, FieldKind::Text | FieldKind::Password | FieldKind::TextArea) || f.value == f.initial { return; }
        f.initial = f.value.clone();
        let node = f.node;
        if let Some(h) = &self.live { h.post(node, "change", "{}"); }
    }

    fn submit(&mut self, proxy: &EventLoopProxy<Msg>, field: usize, submitter: Option<usize>) {
        self.commit_change(field);
        let Some(f) = self.fields[field].form else { return };
        if let (Some(h), Some(form)) = (&self.live, self.page.forms.get(f)) { if h.event(form.node, "submit", "{}") { return; } } // a handler called preventDefault()
        let area_values: Vec<String> = self.fields.iter().map(|fl| if fl.kind == FieldKind::TextArea { fl.value.replace('\n', "\r\n") } else { String::new() }).collect();
        let mut pairs: Vec<(&str, &str)> = vec![];
        for (i, fl) in self.fields.iter().enumerate().filter(|(_, fl)| fl.form == Some(f) && !fl.name.is_empty()) {
            match fl.kind {
                FieldKind::Text | FieldKind::Password | FieldKind::Hidden | FieldKind::Select => pairs.push((&fl.name, &fl.value)),
                FieldKind::TextArea => pairs.push((&fl.name, &area_values[i])),
                FieldKind::Check | FieldKind::Radio => if fl.checked { pairs.push((&fl.name, if fl.value.is_empty() { "on" } else { &fl.value })) },
                FieldKind::Submit => if Some(i) == submitter { pairs.push((&fl.name, &fl.value)) },
            }
        }
        let (url, body) = form_request(&self.page.forms[f], &pairs);
        self.navigate(proxy, url, true, body);
    }

    fn set_radio(&mut self, i: usize) {
        let (form, name) = (self.fields[i].form, self.fields[i].name.clone());
        for (k, fl) in self.fields.iter_mut().enumerate() {
            if fl.kind == FieldKind::Radio && fl.form == form && fl.name == name && !name.is_empty() { fl.checked = k == i; }
        }
        self.fields[i].checked = true;
        let group: Vec<usize> = (0..self.fields.len()).filter(|&k| self.fields[k].kind == FieldKind::Radio && self.fields[k].form == form && self.fields[k].name == name && !name.is_empty() || k == i).collect();
        for k in group { self.live_value(k, if k == i { &["click", "input", "change"] } else { &[] }); }
    }

    fn select_option(&mut self, i: usize, sel: usize) {
        let fl = &mut self.fields[i];
        if let Some(o) = fl.options.get(sel) { fl.sel = sel; fl.value = o.0.clone(); }
        self.live_value(i, &["input", "change"]);
    }

    /// Next/previous focusable text-like control in document order (wraps around).
    fn tab_target(&self, back: bool) -> Option<usize> {
        let ok: Vec<usize> = self.placed.iter().filter_map(|p| p.field).filter(|&i| matches!(self.fields[i].kind, FieldKind::Text | FieldKind::Password | FieldKind::TextArea | FieldKind::Select)).collect();
        if ok.is_empty() { return None; }
        let pos = self.focus.and_then(|f| ok.iter().position(|&i| i == f));
        Some(match (pos, back) {
            (None, false) => ok[0],
            (None, true) => ok[ok.len() - 1],
            (Some(p), false) => ok[(p + 1) % ok.len()],
            (Some(p), true) => ok[(p + ok.len() - 1) % ok.len()],
        })
    }

    fn placed_field(&self, i: usize) -> Option<&Placed> { self.placed.iter().find(|p| p.field == Some(i)) }
}

struct App {
    proxy: EventLoopProxy<Msg>,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    fonts: Fonts,
    tabs: Vec<Tab>,
    cur: usize,
    next_uid: u64,
    sc: f32,
    mouse: PhysicalPosition<f64>,
    cursor: CursorIcon,
    mods: ModifiersState,
    exit: bool,
    find: Option<Find>,
    closed: Vec<String>,
    drag: Drag,
    page_focus: bool, // last click was inside the page (Ctrl+A / Ctrl+C act on page text)
    last_click: Option<(std::time::Instant, f32, f32)>,
    pending: (Vec<String>, usize), // tabs of the previous session still to open
    notice: Option<String>, // one-line message at the bottom (cleared by the next key or click)
}

impl App {
    fn redraw(&self) { if let Some(w) = &self.window { w.request_redraw(); } }
    fn t(&mut self) -> &mut Tab { &mut self.tabs[self.cur] }
    fn top(&self) -> f32 { (TAB_H + BAR) * self.sc }

    fn sync_title(&self) {
        if let Some(w) = &self.window {
            let t = &self.tabs[self.cur];
            w.set_title(&if t.title.is_empty() { "chrome-rs".to_string() } else { format!("{} - chrome-rs", t.title) });
        }
    }

    fn new_tab(&mut self, url: Option<String>) {
        self.next_uid += 1;
        let mut t = match &url { Some(_) => Tab::new(self.next_uid), None => Tab::blank(self.next_uid) };
        if let Some(u) = url { t.navigate(&self.proxy, u, true, None); }
        self.tabs.push(t);
        self.cur = self.tabs.len() - 1;
        self.sync_title();
        self.redraw();
    }

    fn close_tab(&mut self, i: usize) {
        if self.tabs.len() == 1 { store::save_session(&[], 0); self.exit = true; return; }
        if store::recordable(&self.tabs[i].url) { self.closed.push(self.tabs[i].url.clone()); }
        self.tabs.remove(i);
        if i < self.cur || self.cur >= self.tabs.len() { self.cur = self.cur.saturating_sub(1); }
        self.sync_title();
        self.redraw();
    }

    fn switch_tab(&mut self, i: usize) {
        self.cur = i % self.tabs.len();
        self.sync_title();
        self.redraw();
    }

    /// Ctrl+P: renders the whole page at A4-ish width into a PDF under the data dir's `prints` folder.
    fn print_pdf(&mut self) {
        let t = &self.tabs[self.cur];
        let (buf_w, name) = (794u32, format!("{}.pdf", t.title.chars().filter(|c| c.is_alphanumeric()).take(40).collect::<String>().trim()));
        let (w, h, buf) = snap::paint(&t.page, buf_w, &mut self.fonts);
        let bytes = snap::pdf(&buf, w, h);
        let dir = store::data_dir().map(|d| d.join("prints"));
        self.notice = Some(match dir.and_then(|d| { std::fs::create_dir_all(&d).ok()?; let p = d.join(if name == ".pdf" { "page.pdf".to_string() } else { name }); std::fs::write(&p, &bytes).ok()?; Some(p) }) {
            Some(p) => format!("Saved PDF: {}", p.display()),
            None => "Could not save the PDF".to_string(),
        });
        self.redraw();
    }

    fn save_session(&self) { store::save_session(&self.tabs.iter().map(|t| t.url.clone()).collect::<Vec<_>>(), self.cur); }

    fn set_zoom(&mut self, z: f32) {
        let t = &mut self.tabs[self.cur];
        let z = z.clamp(0.3, 5.0);
        t.scroll *= z / t.zoom;
        t.target = None;
        t.zoom = z;
        t.dirty = true;
        self.redraw();
    }

    /// (track x, thumb y, thumb h, viewport h, max scroll) of the page scrollbar; None when the page fits.
    fn scrollbar(&self) -> Option<(f32, f32, f32, f32, f32)> {
        let (win, t, top) = (self.window.as_ref()?, &self.tabs[self.cur], self.top());
        let size = win.inner_size();
        let view = size.height as f32 - top - if self.find.is_some() { 32.0 * self.sc } else { 0.0 };
        let max = t.content_h - view;
        if max <= 1.0 || view < 40.0 { return None; }
        let th = (view * view / t.content_h).max(30.0 * self.sc).min(view);
        Some((size.width as f32 - 12.0 * self.sc, top + t.scroll / max * (view - th), th, view, max))
    }

    fn recompute_find(&mut self) {
        let Some(f) = self.find.as_mut() else { return };
        f.matches = browse::find_matches(&self.tabs[self.cur].placed, &f.query, &mut self.fonts);
        f.cur = 0;
        self.find_reveal();
    }

    /// Scrolls the current find match into view (centered).
    fn find_reveal(&mut self) {
        let (top, sc) = (self.top(), self.sc);
        let vh = self.viewport_h();
        let Some(f) = &self.find else { return };
        let t = &mut self.tabs[self.cur];
        if let Some(&(i, _, _)) = f.matches.get(f.cur).and_then(|m| m.parts.first()) {
            let p = &t.placed[i];
            let view = (vh - top - 32.0 * sc).max(40.0);
            if p.base - p.size < t.scroll || p.base > t.scroll + view { t.scroll = (p.base - view / 2.0).max(0.0); t.target = None; }
        }
    }

    /// `CHROME_RS_SELFTEST=1`: drives find, selection, zoom, suggestions, smooth scroll and the scrollbar through real paints, then exits.
    fn selftest(&mut self) {
        self.paint();
        self.find = Some(Find { query: "e".into(), matches: vec![], cur: 0 });
        self.recompute_find();
        let t = &mut self.tabs[self.cur];
        if let Some(r) = browse::all(&t.placed) { t.sel = Some(r); }
        t.sugg = vec![store::Entry { url: "https://x.test/".into(), title: "x".into(), time: 0 }];
        t.sugg_sel = Some(0);
        t.bookmarked = true;
        self.set_zoom(1.5);
        self.paint();
        self.scroll_by(400.0);
        for _ in 0..40 { self.paint(); }
        let sb = self.scrollbar().is_some();
        self.set_zoom(0.5);
        self.paint();
        let (n, h, sel) = (self.find.as_ref().map_or(0, |f| f.matches.len()), self.tabs[self.cur].content_h, self.tabs[self.cur].sel.is_some());
        println!("selftest ok: matches={n} content_h={h:.0} scrollbar={sb} selection={sel} title={:?}", self.tabs[self.cur].title);
        std::process::exit(0);
    }

    fn tab_width(&self, bw: f32) -> f32 { (220.0 * self.sc).min((bw - 40.0 * self.sc) / self.tabs.len() as f32) }

    /// What is under the mouse in the page area.
    fn field_at(&self) -> Option<usize> {
        let (mx, my, top) = (self.mouse.x as f32, self.mouse.y as f32, self.top());
        let t = &self.tabs[self.cur];
        t.placed.iter().find(|p| {
            let y = p.base - t.scroll + top;
            p.field.is_some() && mx >= p.x && mx <= p.x + p.w && my >= y - p.size && my <= y
        }).and_then(|p| p.field)
    }

    fn link_at(&self) -> Option<usize> {
        let (mx, my, top) = (self.mouse.x as f32, self.mouse.y as f32, self.top());
        let t = &self.tabs[self.cur];
        t.placed.iter().find(|p| {
            let y = p.base - t.scroll + top;
            p.link.is_some() && mx >= p.x && mx <= p.x + p.w && my >= y - p.size && my <= y + p.size * 0.3
        }).and_then(|p| p.link)
    }

    /// Wheel over a scroll container that can still move in that direction scrolls it instead of the page; true when it did.
    fn scroll_container(&mut self, dy: f32) -> bool {
        let (mx, my, top, sc) = (self.mouse.x as f32, self.mouse.y as f32, self.top(), self.sc);
        let t = &mut self.tabs[self.cur];
        if my < top { return false; }
        let (px, py, d) = (mx, my - top + t.scroll, dy * sc);
        let best = t.groups.values().filter(|g| px >= g.x && px <= g.x + g.w && py >= g.y && py <= g.y + g.h).filter(|g| {
            let (max, cur) = ((g.content_h - g.h).max(0.0), t.box_scroll.get(&g.node).copied().unwrap_or(0.0));
            max > 0.5 && (d > 0.0 && cur < max - 0.5 || d < 0.0 && cur > 0.5)
        }).min_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h))).copied(); // the innermost one
        let Some(g) = best else { return false };
        let cur = t.box_scroll.get(&g.node).copied().unwrap_or(0.0);
        t.box_scroll.insert(g.node, (cur + d).clamp(0.0, (g.content_h - g.h).max(0.0)));
        t.box_applied = false;
        self.redraw();
        true
    }

    fn scroll_by(&mut self, dy: f32) {
        let sc = self.sc;
        let t = self.t();
        t.target = Some(t.target.unwrap_or(t.scroll) + dy * sc);
        self.redraw();
    }

    /// Scrolls so that field `i` is on screen.
    fn reveal(&mut self, i: usize, viewport_h: f32) {
        let top = self.top();
        let t = &mut self.tabs[self.cur];
        if let Some(p) = t.placed_field(i) {
            let (y0, y1) = (p.base - p.size, p.base);
            let view = (viewport_h - top).max(0.0);
            t.target = None;
            if y0 < t.scroll { t.scroll = (y0 - 40.0).max(0.0); }
            else if y1 > t.scroll + view { t.scroll = y1 - view + 40.0; }
        }
    }

    fn viewport_h(&self) -> f32 { self.window.as_ref().map_or(720.0, |w| w.inner_size().height as f32) }

    fn paint(&mut self) {
        let (Some(win), Some(surf)) = (&self.window, &mut self.surface) else { return };
        let size = win.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        surf.resize(w, h).unwrap();
        let sc = win.scale_factor() as f32;
        self.sc = sc;
        let csc = sc * self.tabs[self.cur].zoom;
        let (bw, bh) = (size.width as usize, size.height as usize);
        let (tab_h, bar_h) = ((TAB_H * sc) as usize, (BAR * sc) as usize);
        let top = tab_h + bar_h;
        let tw = (220.0 * sc).min((bw as f32 - 40.0 * sc) / self.tabs.len() as f32);
        let strip: Vec<(String, bool)> = self.tabs.iter().enumerate().map(|(i, t)| {
            (if t.loading && t.title.is_empty() { "Loading...".to_string() } else { t.title.clone() }, i == self.cur)
        }).collect();
        let find_h = if self.find.is_some() { (32.0 * sc) as usize } else { 0 };
        let t = &mut self.tabs[self.cur];
        // :hover / :active / :focus change what the CSS selects: restyle from the kept DOM when that state moved
        let focus_node = t.focus.and_then(|i| t.fields.get(i)).map_or(0, |f| f.node);
        let want = (t.hover, t.active, focus_node);
        if want != t.applied {
            t.applied = want;
            if let Some(src) = t.page.restyle.clone() {
                css::set_state(want.0, want.1, want.2);
                let np = render::restyled(&src);
                (t.page.items, t.page.images, t.page.bg) = (np.items, np.images, np.bg);
                t.dirty = true;
            }
        }
        let vh = bh.saturating_sub(top + find_h) as u32;
        if t.dirty || t.layout_w != size.width || t.layout_sc != csc || t.layout_vh != vh {
            (t.placed, t.rects, t.content_h) = layout::layout_vh(&t.page.items, size.width as f32, csc, vh as f32, &mut self.fonts);
            t.layout_vh = vh;
            t.pinned = t.placed.iter().enumerate().filter(|(_, p)| !p.st.is_nan()).map(|(i, p)| (false, i, p.base, p.st, p.lim))
                .chain(t.rects.iter().enumerate().filter(|(_, r)| !r.st.is_nan()).map(|(i, r)| (true, i, r.y, r.st, r.lim))).collect();
            t.groups = layout::groups(&t.rects);
            t.moved = t.placed.iter().enumerate().filter(|(_, p)| p.grp != 0).map(|(i, p)| (false, i, p.base, p.grp))
                .chain(t.rects.iter().enumerate().filter(|(_, r)| r.grp != 0 && r.scroll.is_nan()).map(|(i, r)| (true, i, r.y, r.grp))).collect();
            t.box_applied = false;
            t.pinned_at = f32::NAN;
            t.layout_w = size.width;
            t.layout_sc = csc;
            t.dirty = false;
            if let Some(f) = self.find.as_mut() { f.matches = browse::find_matches(&t.placed, &f.query, &mut self.fonts); f.cur = f.cur.min(f.matches.len().saturating_sub(1)); }
            if let Some(h) = &t.live {
                h.resize((size.width as f32 / csc) as u32, (bh.saturating_sub(top) as f32 / csc) as u32);
                if let Ok(j) = serde_json::to_string(&browse::node_boxes(&t.placed, &t.rects, &t.page.parents, csc)) { h.boxes(j); }
            }
            if std::env::var_os("CHROME_RS_DEBUG").is_some() {
                for p in t.placed.iter().take(12) { eprintln!("x={:.0} base={:.0} size={:.0} w={:.0} img={:?} {:?}", p.x, p.base, p.size, p.w, p.img, p.text); }
            }
        }
        let max_scroll = (t.content_h - (bh.saturating_sub(top + find_h)) as f32).max(0.0);
        let mut animating = false;
        if let Some(tg) = t.target {
            let tg = tg.clamp(0.0, max_scroll);
            let d = tg - t.scroll;
            if d.abs() < 1.0 { t.scroll = tg; t.target = None; } else { t.scroll += d * 0.35; animating = true; }
        }
        t.scroll = t.scroll.clamp(0.0, max_scroll);
        // fixed and sticky boxes follow the viewport: move them in page coordinates so painting and hit-testing agree
        if !t.pinned.is_empty() && t.pinned_at != t.scroll {
            for &(is_rect, i, orig, st, lim) in &t.pinned {
                let v = orig + (t.scroll - st).max(0.0).min(if lim.is_nan() { f32::MAX } else { lim });
                if is_rect { t.rects[i].y = v; } else { t.placed[i].base = v; }
            }
            t.pinned_at = t.scroll;
        }
        // scroll containers: content moves by the container's offset; painting clips it to the box
        if !t.box_applied {
            let off = |g: u32| t.groups.get(&g).map_or(0.0, |gb| t.box_scroll.get(&gb.node).copied().unwrap_or(0.0).clamp(0.0, (gb.content_h - gb.h).max(0.0)));
            let offs: Vec<(bool, usize, f32)> = t.moved.iter().map(|&(is_rect, i, orig, g)| (is_rect, i, orig - off(g))).collect();
            for (is_rect, i, v) in offs { if is_rect { t.rects[i].y = v; } else { t.placed[i].base = v; } }
            t.box_applied = true;
        }
        if let Some(h) = &t.live {
            if (t.scroll - t.scroll_sent).abs() > 1.0 && t.scroll_at.elapsed().as_millis() > 100 { h.scroll(t.scroll / csc); t.scroll_sent = t.scroll; t.scroll_at = std::time::Instant::now(); }
        }
        let mut buf = surf.buffer_mut().unwrap();
        buf.fill(t.page.bg);
        let dy = top as f32 - t.scroll;
        // paint layers: negative z behind the page text, positioned boxes above it, `fixed` boxes last and at screen coordinates
        let box_clip = |g: u32| t.groups.get(&g).map(|b| (b.x as i32, (b.y + dy) as i32, (b.x + b.w).ceil() as i32, (b.y + b.h + dy).ceil() as i32));
        for (z, rr, pr) in layout::layers(&t.placed, &t.rects) {
            let dz = dy;
            for r in &t.rects[rr.clone()] {
                let y = r.y + dz;
                if y + r.h >= top as f32 && y < bh as f32 {
                    gfx::set_clip(box_clip(r.grp).filter(|_| r.scroll.is_nan()));
                    gfx::draw_rect(&mut buf, bw, bh, r, dz);
                    if let Some(pic) = &r.pic { gfx::draw_pic(&mut buf, bw, bh, &t.page.images[pic.img], r, dz); }
                }
            }
            gfx::set_clip(None);
            if z == 0 {
                if let Some(f) = &self.find {
                    for (k, m) in f.matches.iter().enumerate() {
                        for &(i, off, w) in &m.parts {
                            let p = &t.placed[i];
                            fill(&mut buf, bw, (p.x + off) as i32, (p.base - p.size + dy) as i32, w as i32 + 1, (p.size * 1.25) as i32, if k == f.cur { 0xFFA040 } else { 0xFFE066 });
                        }
                    }
                }
                if let Some((a, b)) = t.sel {
                    for (x, y, w, h) in browse::selection_rects(&t.placed, a, b, &mut self.fonts) { fill(&mut buf, bw, x as i32, (y + dy) as i32, w as i32 + 1, h as i32, 0xB3D4FF); }
                }
            }
            for p in &t.placed[pr] {
                let y = p.base + dz;
                if y < top as f32 || y - p.size > bh as f32 { continue; }
                gfx::set_clip(box_clip(p.grp));
                if let Some(i) = p.img {
                    draw_image(&mut buf, bw, bh, &t.page.images[i], p.x, y - p.size, p.w, p.size, if p.color == 0 { 255 } else { p.color });
                    continue;
                }
                if let Some(i) = p.field {
                    let caret = (t.focus == Some(i)).then_some(t.caret);
                    draw_field(&mut buf, bw, bh, &mut self.fonts, &t.fields[i], caret, csc, p.x, y - p.size, p.w, p.size);
                    continue;
                }
                gfx::draw_text_ls(&mut buf, bw, bh, &mut self.fonts, &p.text, p.x, y, p.size, p.color, p.face, p.ls);
                if p.ul { fill(&mut buf, bw, p.x as i32, y as i32 + 2, p.w as i32, 1, p.color); }
            }
            gfx::set_clip(None);
            for r in t.rects[rr].iter().filter(|r| r.scroll.is_finite()) {
                gfx::draw_box_scrollbar(&mut buf, bw, bh, r.x, r.y + dz, r.w, r.h, r.scroll, t.box_scroll.get(&r.node).copied().unwrap_or(0.0), csc);
            }
        }
        if let Some(i) = t.popup {
            if let Some(p) = t.placed_field(i) {
                let y = p.base - t.scroll + top as f32;
                draw_popup(&mut buf, bw, bh, &mut self.fonts, &t.fields[i], p.x, y, p.w, p.size);
            }
        }

        // page scrollbar
        let view = (bh.saturating_sub(top + find_h)) as f32;
        if max_scroll > 1.0 && view > 40.0 {
            let th = (view * view / t.content_h).max(30.0 * sc).min(view);
            let ty = top as f32 + t.scroll / max_scroll * (view - th);
            gfx::draw_rrect(&mut buf, bw, bh, bw as f32 - 9.0 * sc, ty, 6.0 * sc, th, 3.0 * sc, 0.0, 0x9A9A9A);
        }

        // find bar
        if let Some(f) = &self.find {
            let y0 = bh - find_h;
            fill(&mut buf, bw, 0, y0 as i32, bw as i32, find_h as i32, 0xE8E8E8);
            fill(&mut buf, bw, 0, y0 as i32, bw as i32, 1, 0xB0B0B0);
            let label = if f.query.is_empty() { String::new() } else if f.matches.is_empty() { "no matches".to_string() } else { format!("{}/{}", f.cur + 1, f.matches.len()) };
            let text = format!("Find: {}|", f.query);
            let fs = 15.0 * sc;
            draw_text(&mut buf, bw, bh, &mut self.fonts, &text, 12.0 * sc, y0 as f32 + 21.0 * sc, fs, 0x111111);
            let lw = self.fonts.width(&label, fs);
            draw_text(&mut buf, bw, bh, &mut self.fonts, &label, bw as f32 - lw - 24.0 * sc, y0 as f32 + 21.0 * sc, fs, 0x555555);
        }

        if let Some(n) = &self.notice {
            let y0 = bh.saturating_sub((30.0 * sc) as usize + find_h);
            fill(&mut buf, bw, 0, y0 as i32, bw as i32, (30.0 * sc) as i32, 0x333333);
            draw_text(&mut buf, bw, bh, &mut self.fonts, n, 12.0 * sc, y0 as f32 + 20.0 * sc, 14.0 * sc, 0xFFFFFF);
        }

        // tab strip
        fill(&mut buf, bw, 0, 0, bw as i32, tab_h as i32, 0xC8C8C8);
        let fs = 13.0 * sc;
        for (i, (title, active)) in strip.iter().enumerate() {
            let x = i as f32 * tw;
            fill(&mut buf, bw, x as i32 + 1, (4.0 * sc) as i32, tw as i32 - 2, tab_h as i32 - (4.0 * sc) as i32, if *active { 0xE8E8E8 } else { 0xB8B8B8 });
            let mut label = title.clone();
            while label.chars().count() > 1 && self.fonts.width(&label, fs) > tw - 34.0 * sc { label.pop(); }
            draw_text(&mut buf, bw, bh, &mut self.fonts, &label, x + 10.0 * sc, 21.0 * sc, fs, 0x111111);
            draw_text(&mut buf, bw, bh, &mut self.fonts, "x", x + tw - 18.0 * sc, 21.0 * sc, fs, 0x444444);
        }
        draw_text(&mut buf, bw, bh, &mut self.fonts, "+", strip.len() as f32 * tw + 11.0 * sc, 23.0 * sc, 18.0 * sc, 0x333333);

        // address bar
        fill(&mut buf, bw, 0, tab_h as i32, bw as i32, bar_h as i32, 0xE8E8E8);
        let (tx, ty, ts) = (10.0 * sc, tab_h as f32 + 26.0 * sc, 16.0 * sc);
        if t.fresh {
            let w = self.fonts.width(&t.input, ts);
            fill(&mut buf, bw, tx as i32, tab_h as i32 + (8.0 * sc) as i32, w as i32 + 1, (24.0 * sc) as i32, 0xB3D4FF);
        }
        draw_text(&mut buf, bw, bh, &mut self.fonts, &t.input, tx, ty, ts, 0x000000);
        if t.loading {
            let msg = "Loading...";
            let w = self.fonts.width(msg, ts);
            draw_text(&mut buf, bw, bh, &mut self.fonts, msg, bw as f32 - w - tx, ty, ts, 0x666666);
        } else if !t.fresh && t.focus.is_none() {
            let w = self.fonts.width(&t.input, ts);
            fill(&mut buf, bw, (tx + w) as i32 + 1, tab_h as i32 + (8.0 * sc) as i32, 1, (24.0 * sc) as i32, 0x000000);
        }
        // right side of the address bar: zoom level and bookmark star
        let mut rx = bw as f32 - 14.0 * sc;
        if t.bookmarked { let w = self.fonts.width("\u{2605}", ts); rx -= w; draw_text(&mut buf, bw, bh, &mut self.fonts, "\u{2605}", rx, ty, ts, 0xE8A400); rx -= 10.0 * sc; }
        if (t.zoom - 1.0).abs() > 0.001 && !t.loading { let z = format!("{}%", (t.zoom * 100.0).round()); let w = self.fonts.width(&z, ts * 0.85); draw_text(&mut buf, bw, bh, &mut self.fonts, &z, rx - w, ty, ts * 0.85, 0x555555); }
        // address suggestions
        if !t.sugg.is_empty() {
            let row = 26.0 * sc;
            let bx = 8.0 * sc;
            let width = (bw as f32 - 2.0 * bx).min(900.0 * sc);
            fill(&mut buf, bw, bx as i32 - 1, top as i32, width as i32 + 2, (row * t.sugg.len() as f32) as i32 + 1, 0x999999);
            for (i, e) in t.sugg.iter().enumerate() {
                let y = top as f32 + i as f32 * row;
                fill(&mut buf, bw, bx as i32, y as i32, width as i32, row as i32 - if i + 1 == t.sugg.len() { 0 } else { 1 }, if t.sugg_sel == Some(i) { 0xD6E6FF } else { 0xFFFFFF });
                let label = if e.title.is_empty() { e.url.clone() } else { format!("{}  -  {}", e.title, e.url) };
                let mut label = label;
                while label.chars().count() > 1 && self.fonts.width(&label, 14.0 * sc) > width - 16.0 * sc { label.pop(); }
                draw_text(&mut buf, bw, bh, &mut self.fonts, &label, bx + 8.0 * sc, y + 18.0 * sc, 14.0 * sc, 0x111111);
            }
        }
        buf.present().unwrap();
        if animating { win.request_redraw(); }
    }

    fn key(&mut self, key: &Key, text: Option<&str>) {
        self.notice = None;
        let (ctrl, alt, shift) = (self.mods.control_key(), self.mods.alt_key(), self.mods.shift_key());
        let proxy = self.proxy.clone();
        let vh = self.viewport_h();
        let ntabs = self.tabs.len();
        // find bar owns plain typing while it is open
        if self.find.is_some() && !ctrl && !alt {
            match key {
                Key::Named(NamedKey::Escape) => { self.find = None; }
                Key::Named(NamedKey::Enter) => if let Some(f) = self.find.as_mut().filter(|f| !f.matches.is_empty()) {
                    f.cur = if shift { (f.cur + f.matches.len() - 1) % f.matches.len() } else { (f.cur + 1) % f.matches.len() };
                    self.find_reveal();
                },
                Key::Named(NamedKey::Backspace) => { if let Some(f) = self.find.as_mut() { f.query.pop(); } self.recompute_find(); }
                _ => if let Some(s) = text.filter(|s| !s.is_empty() && !s.chars().any(char::is_control)) {
                    if let Some(f) = self.find.as_mut() { f.query.push_str(s); }
                    self.recompute_find();
                } else if !matches!(key, Key::Named(NamedKey::PageDown | NamedKey::PageUp | NamedKey::ArrowDown | NamedKey::ArrowUp | NamedKey::Home | NamedKey::End | NamedKey::F5)) { return; },
            }
            if matches!(key, Key::Named(NamedKey::Escape | NamedKey::Enter | NamedKey::Backspace)) || text.is_some_and(|s| !s.chars().any(char::is_control) && !s.is_empty()) { self.redraw(); return; }
        }
        // browser-level shortcuts first
        match key {
            Key::Named(NamedKey::F3) => if let Some(f) = self.find.as_mut().filter(|f| !f.matches.is_empty()) {
                f.cur = if shift { (f.cur + f.matches.len() - 1) % f.matches.len() } else { (f.cur + 1) % f.matches.len() };
                self.find_reveal();
                self.redraw();
                return;
            },
            Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
                "t" if shift => { if let Some(u) = self.closed.pop() { self.new_tab(Some(u)); } return; }
                "t" => { self.new_tab(None); return; }
                "w" => { self.close_tab(self.cur); return; }
                "f" => { if self.find.is_none() { self.find = Some(Find { query: String::new(), matches: vec![], cur: 0 }); } self.redraw(); return; }
                "g" => if let Some(f) = self.find.as_mut().filter(|f| !f.matches.is_empty()) {
                    f.cur = if shift { (f.cur + f.matches.len() - 1) % f.matches.len() } else { (f.cur + 1) % f.matches.len() };
                    self.find_reveal();
                    self.redraw();
                    return;
                },
                "p" => { self.print_pdf(); return; }
                "d" => { let t = &mut self.tabs[self.cur]; t.bookmarked = store::toggle_bookmark(&t.url.clone(), &t.title.clone()); self.redraw(); return; }
                "h" => { self.new_tab(Some("about:history".into())); return; }
                "b" => { self.new_tab(Some("about:bookmarks".into())); return; }
                "=" | "+" => { let z = zoom_step(self.tabs[self.cur].zoom, true); self.set_zoom(z); return; }
                "-" | "_" => { let z = zoom_step(self.tabs[self.cur].zoom, false); self.set_zoom(z); return; }
                "0" => { self.set_zoom(1.0); return; }
                d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() && d != "0" => { let n = (d.as_bytes()[0] - b'1') as usize; if n < ntabs { self.switch_tab(n); } return; }
                _ => {}
            },
            Key::Named(NamedKey::Tab) if ctrl => { self.switch_tab(if shift { self.cur + ntabs - 1 } else { self.cur + 1 }); return; }
            Key::Named(NamedKey::PageDown) if ctrl => { self.switch_tab(self.cur + 1); return; }
            Key::Named(NamedKey::PageUp) if ctrl => { self.switch_tab(self.cur + ntabs - 1); return; }
            _ => {}
        }
        let t = &mut self.tabs[self.cur];
        // the page sees plain key presses (not browser shortcuts); Enter in a field can be vetoed before it submits
        let mut enter_vetoed = false;
        if let (Some(h), false, false) = (&t.live, ctrl, alt) {
            if let Some(k) = key_name(key).filter(|_| t.focus.is_some() || self.page_focus || t.page.body != 0 && matches!(key, Key::Named(NamedKey::Escape))) {
                let init = serde_json::json!({ "key": k, "shiftKey": shift, "ctrlKey": ctrl, "altKey": alt }).to_string();
                if matches!(key, Key::Named(NamedKey::Enter)) { enter_vetoed = h.event(t.key_target(), "keydown", &init); if !enter_vetoed { h.post(t.key_target(), "keypress", &init); } } else { h.post(t.key_target(), "keydown", &init); if matches!(key, Key::Character(_)) { h.post(t.key_target(), "keypress", &init); } }
            }
        }
        if enter_vetoed { self.redraw(); return; }
        // a focused page field gets editing keys first
        if let Some(i) = t.focus {
            let kind = t.fields[i].kind;
            if t.popup.is_some() {
                match key {
                    Key::Named(NamedKey::Escape | NamedKey::Enter) => t.popup = None,
                    Key::Named(NamedKey::ArrowDown) => { let s = (t.fields[i].sel + 1).min(t.fields[i].options.len().saturating_sub(1)); t.select_option(i, s); }
                    Key::Named(NamedKey::ArrowUp) => { let s = t.fields[i].sel.saturating_sub(1); t.select_option(i, s); }
                    _ => {}
                }
                self.redraw();
                return;
            }
            if kind == FieldKind::Select {
                match key {
                    Key::Named(NamedKey::ArrowDown) => { let s = (t.fields[i].sel + 1).min(t.fields[i].options.len().saturating_sub(1)); t.select_option(i, s); self.redraw(); return; }
                    Key::Named(NamedKey::ArrowUp) => { let s = t.fields[i].sel.saturating_sub(1); t.select_option(i, s); self.redraw(); return; }
                    Key::Named(NamedKey::Space) => { t.popup = Some(i); self.redraw(); return; }
                    Key::Named(NamedKey::Enter) => { t.submit(&proxy, i, None); self.redraw(); return; }
                    _ => {}
                }
            } else if !ctrl && !alt {
                let mut caret = t.caret;
                let handled = match key {
                    Key::Named(NamedKey::Enter) if kind != FieldKind::TextArea => false,
                    _ => edit_field(&mut t.fields[i], &mut caret, key, text),
                };
                t.caret = caret;
                if handled { t.live_value(i, &["input"]); self.redraw(); return; }
            }
        }
        match key {
            Key::Named(NamedKey::Tab) => {
                let t = &mut self.tabs[self.cur];
                if let Some(i) = t.tab_target(shift) {
                    t.set_focus(Some(i));
                    t.popup = None;
                    t.caret = t.fields[i].value.chars().count();
                    self.reveal(i, vh);
                }
            }
            Key::Named(NamedKey::Enter) => {
                let t = &mut self.tabs[self.cur];
                match t.focus {
                    Some(i) => t.submit(&proxy, i, None),
                    None => {
                        let u = t.sugg_sel.and_then(|i| t.sugg.get(i)).map_or_else(|| to_url(&t.input), |e| e.url.clone());
                        t.sugg.clear();
                        t.navigate(&proxy, u, true, None);
                    }
                }
            }
            Key::Named(NamedKey::ArrowDown) if !self.tabs[self.cur].sugg.is_empty() && self.tabs[self.cur].focus.is_none() => {
                let t = &mut self.tabs[self.cur];
                t.sugg_sel = Some(t.sugg_sel.map_or(0, |i| (i + 1).min(t.sugg.len() - 1)));
            }
            Key::Named(NamedKey::ArrowUp) if !self.tabs[self.cur].sugg.is_empty() && self.tabs[self.cur].focus.is_none() => {
                let t = &mut self.tabs[self.cur];
                t.sugg_sel = t.sugg_sel.and_then(|i| i.checked_sub(1));
            }
            Key::Named(NamedKey::Escape) => {
                let t = &mut self.tabs[self.cur];
                t.sugg.clear();
                if t.focus.take().is_none() { t.input = t.url.clone(); t.fresh = false; }
            }
            Key::Named(NamedKey::Backspace) => {
                let t = &mut self.tabs[self.cur];
                if t.fresh { t.input.clear(); t.fresh = false } else { t.input.pop(); }
                t.refresh_sugg();
            }
            Key::Named(NamedKey::ArrowLeft) if alt => self.tabs[self.cur].history(&proxy, true),
            Key::Named(NamedKey::ArrowRight) if alt => self.tabs[self.cur].history(&proxy, false),
            Key::Named(NamedKey::F5) => { let t = &mut self.tabs[self.cur]; let u = t.url.clone(); t.navigate(&proxy, u, false, None); }
            Key::Named(NamedKey::ArrowDown) => self.scroll_by(48.0),
            Key::Named(NamedKey::ArrowUp) => self.scroll_by(-48.0),
            Key::Named(NamedKey::PageDown) => self.scroll_by(500.0),
            Key::Named(NamedKey::PageUp) => self.scroll_by(-500.0),
            Key::Named(NamedKey::Home) => { let t = &mut self.tabs[self.cur]; t.scroll = 0.0; t.target = None; }
            Key::Named(NamedKey::End) => { let t = &mut self.tabs[self.cur]; t.scroll = f32::MAX; t.target = None; }
            Key::Character(c) if ctrl => {
                let t = &mut self.tabs[self.cur];
                match c.as_str() {
                    "l" => { t.fresh = true; t.focus = None; self.page_focus = false; }
                    "a" => if t.focus.is_none() {
                        match browse::all(&t.placed).filter(|_| self.page_focus) { Some(r) => t.sel = Some(r), None => t.fresh = true }
                    },
                    "c" => {
                        let s = match (t.focus, t.sel) {
                            (Some(i), _) => t.fields[i].value.clone(),
                            (None, Some((a, b))) if self.page_focus => browse::selected_text(&t.placed, a, b),
                            _ => t.input.clone(),
                        };
                        let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(s));
                    }
                    "v" => if let Ok(s) = arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
                        match t.focus {
                            Some(i) if matches!(t.fields[i].kind, FieldKind::Text | FieldKind::Password | FieldKind::TextArea) => {
                                let s: String = if t.fields[i].kind == FieldKind::TextArea { s.replace("\r\n", "\n").chars().filter(|c| *c == '\n' || !c.is_control()).collect() } else { s.lines().next().unwrap_or("").chars().filter(|c| !c.is_control()).collect() };
                                let mut caret = t.caret;
                                let at = byte_at(&t.fields[i].value, caret);
                                t.fields[i].value.insert_str(at, &s);
                                caret += s.chars().count();
                                t.caret = caret;
                                t.live_value(i, &["input"]);
                            }
                            Some(_) => {}
                            None => {
                                let s: String = s.lines().next().unwrap_or("").chars().filter(|c| !c.is_control()).collect();
                                if t.fresh { t.input.clear(); t.fresh = false; }
                                t.input.push_str(&s);
                                t.refresh_sugg();
                            }
                        }
                    },
                    "r" => { let u = t.url.clone(); t.navigate(&proxy, u, false, None); }
                    _ => {}
                }
            }
            _ if !ctrl => if let Some(s) = text.filter(|s| !s.chars().any(char::is_control)) {
                let t = &mut self.tabs[self.cur];
                if t.focus.is_none() {
                    if t.fresh { t.input.clear(); t.fresh = false; }
                    t.input.push_str(s);
                    t.refresh_sugg();
                }
            },
            _ => {}
        }
        self.redraw();
    }

    fn click(&mut self) {
        let (mx, my) = (self.mouse.x as f32, self.mouse.y as f32);
        let sc = self.sc;
        let proxy = self.proxy.clone();
        let (tab_h, top) = (TAB_H * sc, self.top());
        let bw = self.window.as_ref().map_or(1000.0, |w| w.inner_size().width as f32);
        if my < tab_h {
            let tw = self.tab_width(bw);
            let n = self.tabs.len();
            let i = (mx / tw) as usize;
            if i < n {
                if mx - i as f32 * tw > tw - 26.0 * sc { self.close_tab(i); } else { self.switch_tab(i); }
            } else if mx < n as f32 * tw + 36.0 * sc { self.new_tab(None); }
            return;
        }
        // address suggestions sit just under the bar
        let n_sugg = self.tabs[self.cur].sugg.len();
        if n_sugg > 0 && my >= top && my < top + 26.0 * sc * n_sugg as f32 {
            let t = &mut self.tabs[self.cur];
            let u = t.sugg[((my - top) / (26.0 * sc)) as usize].url.clone();
            t.sugg.clear();
            t.navigate(&proxy, u, true, None);
            self.redraw();
            return;
        }
        self.tabs[self.cur].sugg.clear();
        if my < top {
            self.page_focus = false;
            let t = self.t();
            t.fresh = true; t.focus = None; t.popup = None;
            self.redraw();
            return;
        }
        // open select list gets the click first
        let popup = self.tabs[self.cur].popup;
        if let Some(i) = popup {
            let t = &mut self.tabs[self.cur];
            t.popup = None;
            if let Some(p) = t.placed_field(i) {
                let y = p.base - t.scroll + top;
                let n = t.fields[i].options.len().min(POPUP_ROWS);
                let first = t.fields[i].sel.saturating_sub(POPUP_ROWS - 1).min(t.fields[i].options.len().saturating_sub(POPUP_ROWS));
                if mx >= p.x && mx <= p.x + p.w && my >= y && my < y + p.size * n as f32 {
                    let row = ((my - y) / p.size) as usize;
                    t.select_option(i, first + row);
                }
            }
            self.redraw();
            return;
        }
        if let Some(i) = self.field_at() {
            let t = &mut self.tabs[self.cur];
            match t.fields[i].kind {
                FieldKind::Text | FieldKind::Password | FieldKind::TextArea => {
                    let (pw, ph, px, py) = t.placed_field(i).map_or((0.0, 0.0, 0.0, 0.0), |p| (p.w, p.size, p.x, p.base - t.scroll + top - p.size));
                    t.set_focus(Some(i));
                    t.caret = gfx_hit_caret(&mut self.fonts, &t.fields[i], sc * t.zoom, pw, ph, mx - px, my - py, if t.focus == Some(i) { t.caret } else { usize::MAX });
                }
                FieldKind::Select => { t.set_focus(Some(i)); t.popup = Some(i); }
                FieldKind::Submit => {
                    let node = t.fields[i].node;
                    let vetoed = t.live.as_ref().is_some_and(|h| h.event(node, "click", r#"{"button":0}"#));
                    if !vetoed { t.submit(&proxy, i, Some(i)); }
                }
                FieldKind::Check => { t.fields[i].checked ^= true; t.set_focus(None); t.live_value(i, &["click", "input", "change"]); }
                FieldKind::Radio => { t.set_focus(None); t.set_radio(i); }
                FieldKind::Hidden => {}
            }
        } else {
            self.tabs[self.cur].set_focus(None);
            // the page's click handlers run first and may cancel the default action (following a link)
            let (csc, top) = (sc * self.tabs[self.cur].zoom, self.top());
            let t = &self.tabs[self.cur];
            let prevented = t.live.as_ref().is_some_and(|h| {
                let node = Some(browse::hit_node(&t.placed, &t.rects, mx, my - top + t.scroll)).filter(|&n| n != 0).unwrap_or(t.page.body);
                let init = serde_json::json!({ "clientX": mx / csc, "clientY": (my - top) / csc, "button": 0, "ctrlKey": self.mods.control_key(), "shiftKey": self.mods.shift_key() }).to_string();
                h.post(node, "mousedown", &init);
                h.post(node, "mouseup", &init);
                h.event(node, "click", &init)
            });
            if prevented { self.redraw(); return; }
            let link = self.link_at().and_then(|i| self.tabs[self.cur].page.links.get(i)).cloned();
            if let Some(node) = link.as_deref().and_then(|u| u.strip_prefix("label:")).and_then(|n| n.parse::<usize>().ok()) {
                // clicking a <label> acts on its control
                let t = &mut self.tabs[self.cur];
                if let Some(i) = t.fields.iter().position(|f| f.node == node) {
                    match t.fields[i].kind {
                        FieldKind::Text | FieldKind::Password | FieldKind::TextArea => { t.set_focus(Some(i)); t.caret = t.fields[i].value.chars().count(); }
                        FieldKind::Select => { t.set_focus(Some(i)); t.popup = Some(i); }
                        FieldKind::Submit => t.submit(&proxy, i, Some(i)),
                        FieldKind::Check => { t.fields[i].checked ^= true; t.live_value(i, &["click", "input", "change"]); }
                        FieldKind::Radio => t.set_radio(i),
                        FieldKind::Hidden => {}
                    }
                }
            } else if let Some(u) = link.filter(|u| u.starts_with("http") || u.starts_with("file:") || u.starts_with("data:")) {
                if self.mods.control_key() { self.new_tab(Some(u)); } else { self.tabs[self.cur].navigate(&proxy, u, true, None); }
            }
        }
        self.redraw();
    }

    /// Left button pressed: scrollbar, text selection start (links fire on release), or a normal click.
    fn mouse_down(&mut self) {
        self.notice = None;
        let (mx, my, top, sc) = (self.mouse.x as f32, self.mouse.y as f32, self.top(), self.sc);
        let now = std::time::Instant::now();
        let double = self.last_click.is_some_and(|(t, x, y)| now.duration_since(t).as_millis() < 450 && (x - mx).abs() < 5.0 && (y - my).abs() < 5.0);
        self.last_click = Some((now, mx, my));
        self.drag = Drag::None;
        if my >= top {
            if let Some((tx, ty, th, view, _)) = self.scrollbar() {
                if mx >= tx {
                    if my >= ty && my <= ty + th { self.drag = Drag::Scrollbar { grab: my - ty }; } else { self.scroll_by(if my < ty { -view * 0.9 / sc } else { view * 0.9 / sc }); }
                    return;
                }
            }
        }
        let t = &mut self.tabs[self.cur];
        if t.page.restyle.is_some() && my >= top { t.active = t.hover; self.redraw(); }
        let t = &mut self.tabs[self.cur];
        let in_page = my >= top && t.popup.is_none();
        let on_field = in_page && self.field_at().is_some();
        let t = &mut self.tabs[self.cur];
        t.sel = None;
        if in_page && !on_field && t.sugg.is_empty() {
            self.page_focus = true;
            t.focus = None;
            let (lx, ly) = (mx, my - top + t.scroll);
            if let Some(pt) = browse::hit_point(&t.placed, lx, ly, &mut self.fonts) {
                if double { t.sel = Some(browse::word_at(&t.placed, pt)); self.redraw(); return; }
                self.drag = Drag::Select { anchor: pt, moved: false, down: (mx, my) };
                self.redraw();
                return;
            }
        }
        self.click();
    }

    fn mouse_move(&mut self) {
        let (mx, my, top) = (self.mouse.x as f32, self.mouse.y as f32, self.top());
        match &mut self.drag {
            Drag::Select { anchor, moved, down } => {
                if !*moved && (mx - down.0).abs() + (my - down.1).abs() < 5.0 { return; }
                *moved = true;
                let anchor = *anchor;
                let t = &mut self.tabs[self.cur];
                if let Some(pt) = browse::hit_point(&t.placed, mx, my - top + t.scroll, &mut self.fonts) { t.sel = Some((anchor, pt)); }
                self.redraw();
            }
            Drag::Scrollbar { grab } => {
                let grab = *grab;
                if let Some((_, _, th, view, max)) = self.scrollbar() {
                    let t = &mut self.tabs[self.cur];
                    t.scroll = ((my - grab - top) / (view - th).max(1.0) * max).clamp(0.0, max);
                    t.target = None;
                    self.redraw();
                }
            }
            Drag::None => {
                let t = &mut self.tabs[self.cur];
                if t.page.restyle.is_some() && my >= top {
                    let n = browse::hit_node(&t.placed, &t.rects, mx, my - top + t.scroll);
                    if n != t.hover { t.hover = n; self.redraw(); }
                }
            }
        }
    }

    fn mouse_up(&mut self) {
        if self.tabs[self.cur].active != 0 { self.tabs[self.cur].active = 0; self.redraw(); }
        if let Drag::Select { moved: false, .. } = self.drag { self.drag = Drag::None; self.click(); return; }
        self.drag = Drag::None;
    }

    fn update_cursor(&mut self) {
        let (mx, my) = (self.mouse.x as f32, self.mouse.y as f32);
        let (tab_h, top) = (TAB_H * self.sc, self.top());
        let icon = if my < tab_h { CursorIcon::Pointer }
            else if my < top { CursorIcon::Text }
            else if let Some(i) = self.field_at() {
                match self.tabs[self.cur].fields[i].kind { FieldKind::Text | FieldKind::Password | FieldKind::TextArea => CursorIcon::Text, _ => CursorIcon::Pointer }
            } else if self.link_at().is_some() { CursorIcon::Pointer }
            else { CursorIcon::Default };
        let _ = mx;
        if icon != self.cursor {
            self.cursor = icon;
            if let Some(w) = &self.window { w.set_cursor(icon); }
        }
    }
}

/// Char index in a text field nearest to a click at (rel_x, rel_y) inside the box; `cur` is the current caret
/// (usize::MAX = unfocused, window anchored at the end) so the click maps through the same scrolled view the user sees.
fn gfx_hit_caret(f: &mut Fonts, fl: &Field, sc: f32, w: f32, h: f32, rel_x: f32, rel_y: f32, cur: usize) -> usize {
    let fs = (fl.fs * sc).round().max(8.0);
    let chars: Vec<char> = if fl.kind == FieldKind::Password { vec!['\u{2022}'; fl.value.chars().count()] } else { fl.value.chars().collect() };
    let cur = cur.min(chars.len());
    let nearest = |f: &mut Fonts, s: usize, e: usize, x: f32| -> usize {
        let mut acc = 0.0;
        for i in s..e {
            let cw = f.width(&chars[i].to_string(), fs);
            if x < acc + cw / 2.0 { return i; }
            acc += cw;
        }
        e
    };
    if fl.kind == FieldKind::TextArea {
        let lines = gfx::wrap_lines(f, &chars, fs, w - 14.0);
        let line_h = fs * 1.35;
        let rows = (((h - 8.0) / line_h) as usize).max(1);
        let cl = lines.iter().rposition(|&(s, _)| s <= cur).unwrap_or(0);
        let first = (cl + 1).saturating_sub(rows);
        let k = (first + ((rel_y - 4.0) / line_h).max(0.0) as usize).min(lines.len() - 1);
        let (s, e) = lines[k];
        return nearest(f, s, e, rel_x - 6.0);
    }
    // same window as draw_field: widen backwards from the caret while it fits
    let max = w - 12.0;
    let (mut s, mut used) = (cur, 0.0);
    while s > 0 { let cw = f.width(&chars[s - 1].to_string(), fs); if used + cw > max { break; } used += cw; s -= 1; }
    nearest(f, s, chars.len(), rel_x - 6.0)
}

impl ApplicationHandler<Msg> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let win = Arc::new(el.create_window(Window::default_attributes().with_title("chrome-rs").with_inner_size(winit::dpi::LogicalSize::new(1000.0, 720.0))).unwrap());
        let ctx = softbuffer::Context::new(win.clone()).unwrap();
        self.surface = Some(softbuffer::Surface::new(&ctx, win.clone()).unwrap());
        self.sc = win.scale_factor() as f32;
        win.set_ime_allowed(true);
        self.window = Some(win);
        let (proxy, t) = (self.proxy.clone(), &mut self.tabs[0]);
        let u = t.url.clone();
        t.navigate(&proxy, u, true, None);
        let (rest, cur) = std::mem::take(&mut self.pending);
        for u in rest { self.new_tab(Some(u)); }
        if cur > 0 { self.switch_tab(cur); }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, msg: Msg) {
        let proxy = self.proxy.clone();
        let (uid, id, url, push, res) = match msg {
            Msg::Loaded { uid, id, url, push, res } => (uid, id, url, push, res),
            Msg::Live { uid, id, ev } => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.uid == uid) {
                    match ev {
                        load::LiveEvent::Page(p) => t.live_update(id, p),
                        load::LiveEvent::Navigate(u) => if id == t.nav_id { t.navigate(&proxy, u, true, None); },
                    }
                }
                self.sync_title();
                self.redraw();
                return;
            }
        };
        let res = res.map(|(u, mut page)| { for w in std::mem::take(&mut page.webfonts) { self.fonts.add_web(w.fam, w.bold, w.italic, &w.data); } (u, page) });
        if let Some(t) = self.tabs.iter_mut().find(|t| t.uid == uid) { t.loaded(id, url, push, res); }
        self.sync_title();
        self.redraw();
        if std::env::var_os("CHROME_RS_SELFTEST").is_some() { self.selftest(); }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => { self.save_session(); el.exit() }
            WindowEvent::RedrawRequested => self.paint(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.redraw(),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::CursorMoved { position, .. } => { self.mouse = position; self.mouse_move(); self.update_cursor(); }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => self.mouse_down(),
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => self.mouse_up(),
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta { MouseScrollDelta::LineDelta(_, y) => -y * 48.0, MouseScrollDelta::PixelDelta(p) => -p.y as f32 / self.sc };
                if self.mods.control_key() { let z = zoom_step(self.tabs[self.cur].zoom, dy < 0.0); self.set_zoom(z); } else if !self.scroll_container(dy) { self.scroll_by(dy); }
            }
            WindowEvent::Ime(winit::event::Ime::Commit(s)) => self.key(&Key::Character(s.as_str().into()), Some(&s)),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => self.key(&event.logical_key, event.text.as_deref()),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Released => {
                let t = &self.tabs[self.cur];
                if let (Some(h), Some(k), false) = (&t.live, key_name(&event.logical_key), self.mods.control_key() || self.mods.alt_key()) {
                    if t.focus.is_some() || self.page_focus { h.post(t.key_target(), "keyup", &serde_json::json!({ "key": k, "shiftKey": self.mods.shift_key() }).to_string()); }
                }
            }
            _ => {}
        }
        if self.exit { el.exit(); }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("--snap" | "--corpus" | "--bench" | "--pdf" | "--save")) {
        // the same stack size as the page loader threads of the browser, so the corpus hits the same limits
        let a = args.clone();
        let code = std::thread::Builder::new().stack_size(32 << 20).spawn(move || snap::cli(&a, &mut Fonts::system().expect("no system font found (see SANS in gfx.rs)"))).ok().and_then(|h| h.join().ok()).unwrap_or(2);
        std::process::exit(code);
    }
    let fonts = Fonts::system().expect("no system font found (see SANS in gfx.rs)");
    let el = EventLoop::<Msg>::with_user_event().build().unwrap();
    let mut first = Tab::new(1);
    let mut pending = (vec![], 0);
    first.url = match args.first() {
        Some(a) => to_url(a),
        None => {
            let (mut urls, cur) = store::load_session(); // reopen the previous session
            if urls.is_empty() { store::setting("home").map_or_else(|| HOME.to_string(), |h| to_url(&h)) } else { let first = urls.remove(0); pending = (urls, cur); first }
        }
    };
    let mut app = App {
        proxy: el.create_proxy(), window: None, surface: None, fonts, tabs: vec![first], cur: 0, next_uid: 1, sc: 1.0,
        mouse: PhysicalPosition::new(0.0, 0.0), cursor: CursorIcon::Default, mods: ModifiersState::empty(), exit: false,
        find: None, closed: vec![], drag: Drag::None, page_focus: false, last_click: None, pending, notice: None,
    };
    el.run_app(&mut app).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(kind: FieldKind, v: &str) -> Field { Field { kind, value: v.into(), fs: 16.0, ..Default::default() } }

    #[test]
    fn form_submission_urls() {
        let get = render::Form { action: "https://a.com/s?old=1#x".into(), post: false, node: 0 };
        assert_eq!(form_request(&get, &[("q", "rust lang"), ("t", "a&b")]), ("https://a.com/s?q=rust%20lang&t=a%26b".into(), None));
        let post = render::Form { action: "https://a.com/login".into(), post: true, node: 0 };
        assert_eq!(form_request(&post, &[("u", "x")]), ("https://a.com/login".into(), Some("u=x".into())));
    }

    #[test]
    fn address_bar() {
        assert_eq!(to_url("example.com"), "https://example.com");
        assert_eq!(to_url("http://a.b/x"), "http://a.b/x");
        assert_eq!(to_url_with("rust lang", ENGINE), "https://html.duckduckgo.com/html/?q=rust%20lang");
        assert_eq!(to_url_with("a b", "https://s.test/?q={q}&x=1"), "https://s.test/?q=a%20b&x=1");
        assert_eq!(to_url("about:history"), "about:history");
    }

    #[test]
    fn caret_editing_with_unicode() {
        let mut f = field(FieldKind::Text, "привет");
        let mut c = 6;
        assert!(edit_field(&mut f, &mut c, &Key::Named(NamedKey::ArrowLeft), None));
        assert!(edit_field(&mut f, &mut c, &Key::Named(NamedKey::Backspace), None));
        assert_eq!((f.value.as_str(), c), ("привт", 4));
        assert!(edit_field(&mut f, &mut c, &Key::Character("x".into()), Some("é")));
        assert_eq!((f.value.as_str(), c), ("привéт", 5));
        assert!(edit_field(&mut f, &mut c, &Key::Named(NamedKey::Home), None));
        assert!(edit_field(&mut f, &mut c, &Key::Named(NamedKey::Delete), None));
        assert_eq!((f.value.as_str(), c), ("ривéт", 0));
        assert!(!edit_field(&mut f, &mut c, &Key::Named(NamedKey::Enter), None)); // Enter submits single-line fields
    }

    #[test]
    fn textarea_lines() {
        let mut f = field(FieldKind::TextArea, "ab\ncde");
        let mut c = 6;
        assert!(edit_field(&mut f, &mut c, &Key::Named(NamedKey::ArrowUp), None));
        assert_eq!(c, 2); // column 3 clamped to the end of "ab"
        assert!(edit_field(&mut f, &mut c, &Key::Named(NamedKey::Enter), None));
        assert_eq!((f.value.as_str(), c), ("ab\n\ncde", 3));
        assert_eq!(line_bounds("ab\n\ncde", 3), (3, 3));
    }

    #[test]
    fn zoom_steps_walk_the_preset_list() {
        assert_eq!(zoom_step(1.0, true), 1.1);
        assert_eq!(zoom_step(1.0, false), 0.9);
        assert_eq!(zoom_step(5.0, true), 5.0);
        assert_eq!(zoom_step(0.3, false), 0.3);
        assert_eq!(zoom_step(1.05, true), 1.1);
    }

    #[test]
    fn live_updates_keep_what_the_user_typed() {
        let mk = |name: &str, value: &str, initial: &str| Field { kind: FieldKind::Text, name: name.into(), value: value.into(), initial: initial.into(), ..Default::default() };
        let old = vec![mk("q", "typed by user", ""), mk("other", "x", "x")];
        let mut new = vec![mk("q", "", ""), mk("other", "set by script", "x")];
        merge_fields(&old, &mut new);
        assert_eq!(new[0].value, "typed by user", "script left the field alone: keep the user's text");
        assert_eq!(new[1].value, "set by script", "script changed the field: the script wins");
        let mut moved = vec![mk("fresh", "", ""), mk("q", "", "")];
        merge_fields(&old, &mut moved);
        assert_eq!(moved[1].value, "typed by user", "a field that moved in the DOM is found by name");
    }
}
