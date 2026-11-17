// Styled DOM -> flat stream of words and breaks that the layout in main.rs flows into lines.
use crate::css::{self, BgSize, Grad, Len, Shadow, Sheet, Style, Track};
use std::sync::Arc;
use crate::dom::Dom;
use crate::load::Img;
use std::collections::HashMap;

const MAX_IMG_CSS_W: f32 = 900.0;

pub enum Item {
    Word { text: String, size: f32, color: u32, bold: bool, italic: bool, fam: u16, ul: bool, mono: bool, link: Option<usize>, space: bool, indent: f32, align: u8, lh: f32, node: usize, ls: f32, dy: f32 },
    /// Inline picture (index into Page::images) at display size w x h in CSS px.
    Image { img: usize, w: f32, h: f32, link: Option<usize>, indent: f32, align: u8, node: usize, va: u8, alpha: f32,
        /// CSS `width: N%`: the layout makes the picture N% of the line and keeps the proportions of `w` x `h`
        pct: Option<f32> },
    /// Form control (index into Page::fields): box of w x h CSS px, `size` = font size.
    Field { idx: usize, w: f32, h: f32, indent: f32, align: u8, node: usize },
    /// Line break plus a vertical gap in px (gaps between consecutive breaks collapse to the largest).
    Break(f32),
    /// Block box (margins, padding, width limits, background) around everything up to the matching BoxEnd.
    Box(BoxSpec),
    BoxEnd,
    /// Table laid out in columns (see layout.rs).
    Table(Box<Table>),
    /// Content that floats to the left/right edge; following lines wrap around it.
    Float(Box<FloatSpec>),
    /// `clear`: continue below floats (bit 0 = left, bit 1 = right).
    Clear(u8),
    /// Horizontal rule in the given color.
    Rule(u32),
    /// `display:inline-block`: laid out as its own block, then placed on the line as one atom (`right` unused).
    InlineBlock(Box<FloatSpec>),
    /// Row flex container.
    Flex(Box<FlexSpec>),
    /// `position:absolute` content, placed against the nearest positioned ancestor (or the page); takes no space in the flow.
    Abs(Box<AbsSpec>),
    /// `position:relative` block: offsets (top, right, bottom, left) shift everything up to the matching RelEnd.
    RelStart([Option<Len>; 4], i32, bool),
    RelEnd,
    /// `display:grid`: children placed in rows over the template's columns.
    Grid(Box<GridSpec>),
}

/// A `background-image: url()` that was downloaded: index into `Page::images`, natural size (CSS px) and how it is placed.
pub struct BgPaint { pub img: usize, pub nw: f32, pub nh: f32, pub size: BgSize, pub pos: [Len; 2], pub repeat: (bool, bool) }

pub struct GridSpec { pub items: Vec<FlexItem>, pub tracks: Vec<Track>, pub auto_min: Option<f32>, pub col_gap: f32, pub row_gap: f32 }

pub struct FlexItem {
    pub items: Vec<Item>, pub grow: f32, pub shrink: f32, pub basis: Option<Len>, pub width: Option<Len>, pub max_width: Option<Len>,
    pub auto_l: bool, pub auto_r: bool, pub align: u8, pub span: usize,
}
pub struct FlexSpec { pub items: Vec<FlexItem>, pub wrap: bool, pub gap: f32, pub justify: u8, pub align: u8, pub col: bool }
pub struct AbsSpec { pub items: Vec<Item>, pub off: [Option<Len>; 4], pub width: Option<Len>, pub max_width: Option<Len>, pub fixed: bool, pub z: i32 }

pub struct FloatSpec { pub right: bool, pub width: Option<Len>, pub max_width: Option<Len>, pub items: Vec<Item> }

#[derive(Clone)]
pub struct BoxSpec {
    pub ml: f32, pub mr: f32, pub pl: f32, pub pr: f32, pub width: Option<Len>, pub max_width: Option<Len>,
    pub auto_l: bool, pub auto_r: bool, pub bg: Option<u32>,
    pub pt: f32, pub pb: f32, // vertical padding inside the box (CSS px)
    pub scroll: bool,         // scroll container: overflow auto|scroll with a fixed height
    pub bgimg: Option<Arc<BgPaint>>,
    pub bg_a: f32, pub clip: bool, pub shadow: Option<Shadow>, pub grad: Option<Arc<Grad>>,
    pub bb: bool, pub min_h: f32, pub radius: f32, pub node: usize,
    pub borders: [Option<(f32, u32)>; 4], // top, right, bottom, left
}

pub struct Table { pub rows: Vec<Row>, pub width: Option<Len>, pub border: f32, pub spacing: f32, pub bg: Option<u32>, pub align: u8 }
pub struct Row { pub cells: Vec<Cell>, pub bg: Option<u32> }
pub struct Cell {
    pub items: Vec<Item>, pub colspan: usize, pub width: Option<Len>, pub bg: Option<u32>,
    pub pad: [f32; 4], // top, right, bottom, left (CSS px)
    pub valign: u8, pub nowrap: bool, pub rowspan: usize,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum FieldKind { #[default] Text, Password, Submit, Hidden, Check, Radio, TextArea, Select }

#[derive(Clone, Default)]
pub struct Field {
    pub form: Option<usize>, pub name: String, pub value: String, pub label: String, pub placeholder: String,
    pub kind: FieldKind, pub checked: bool, pub fs: f32,
    /// value / checked state as parsed (the page's own); used to tell user edits from script changes after a live update
    pub initial: String, pub initial_checked: bool,
    /// DOM node of the control (a `<label>` click is routed here)
    pub node: usize,
    /// Select only: (value, label) per option and the chosen index.
    pub options: Vec<(String, String)>, pub sel: usize,
}

pub struct Form { pub action: String, pub post: bool, pub node: usize }

/// A downloaded web font: decoded TTF/OTF bytes for family id `fam`.
pub struct WebFont { pub fam: u16, pub bold: bool, pub italic: bool, pub data: Vec<u8> }

#[derive(Default)]
pub struct Page {
    pub items: Vec<Item>, pub links: Vec<String>, pub title: String, pub bg: u32, pub images: Vec<Img>,
    pub forms: Vec<Form>, pub fields: Vec<Field>,
    /// `@font-face` rules of the page and the fonts downloaded for them (filled in by the loader)
    pub faces: Vec<css::FontFaceRule>, pub webfonts: Vec<WebFont>,
    /// DOM parent of every node id, the `<body>` node, and the live script session (when scripts may keep running)
    pub parents: Vec<usize>, pub body: usize, pub live: Option<crate::js::Handle>,
    /// What a restyle needs (only for pages whose CSS uses :hover/:active/:focus)
    pub restyle: Option<Arc<Restyle>>,
}

pub struct Restyle { pub dom: Dom, pub imgs: HashMap<usize, Img>, pub base: String }

struct Ctx<'a> {
    dom: &'a Dom,
    sheet: Sheet,
    base: &'a str,
    items: Vec<Item>,
    links: Vec<String>,
    link: Option<usize>,
    pending_space: bool,
    bg: Option<u32>,
    bg_stack: Vec<u32>,
    imgs: HashMap<usize, Img>,
    images: Vec<Img>,
    forms: Vec<Form>,
    fields: Vec<Field>,
    cur_form: Option<usize>,
    indent: f32, // text-indent waiting for the first word of the block
    node: usize, // DOM node being walked (events target it)
    cb_h: Option<f32>, // definite height of the containing block (CSS px), for percentage heights
    counters: Vec<(String, i32)>, // CSS counters in scope (innermost last)
    glob: HashMap<String, i32>,   // counters incremented without a counter-reset
}

/// `c` shown at `alpha` over `bg`.
fn mix(c: u32, bg: u32, alpha: f32) -> u32 {
    let ch = |sh: u32| (((c >> sh & 255) as f32 * alpha + (bg >> sh & 255) as f32 * (1.0 - alpha)).round() as u32).min(255) << sh;
    ch(16) | ch(8) | ch(0)
}

fn backdrop(cx: &Ctx) -> u32 { cx.bg_stack.last().copied().or(cx.bg).unwrap_or(0xFFFFFF) }

fn lum(c: u32) -> f32 { 0.299 * (c >> 16 & 255) as f32 + 0.587 * (c >> 8 & 255) as f32 + 0.114 * (c & 255) as f32 }

fn sheets_of(dom: &Dom) -> Vec<String> {
    let mut sheets = vec![css::UA.to_string()];
    for id in dom.descendants() {
        let n = &dom.nodes[id];
        if n.tag == "style" && !dom.attr(id, "media").is_some_and(|m| m.contains("print")) { sheets.push(dom.text_content(id)); }
    }
    sheets
}

/// Elements whose computed `background-image` is a `url()`, with the absolute address (the loader downloads these).
pub fn background_urls(dom: &Dom, base: &str) -> Vec<(usize, String)> {
    let sheets = sheets_of(dom);
    let styled = dom.nodes.iter().any(|n| n.attrs.iter().any(|(k, v)| k == "style" && v.contains("url(")));
    if !styled && !sheets.iter().any(|s| s.contains("url(")) { return vec![]; }
    let sheet = Sheet::new(&sheets);
    let root = Style::root();
    dom.descendants().into_iter().filter(|&i| !dom.nodes[i].tag.is_empty()).filter_map(|i| {
        let u = css::compute(dom, &sheet, i, &root).bg_img?.url;
        Some((i, if u.starts_with("data:") { u } else { resolve(base, &u) }))
    }).take(24).collect()
}

/// Rebuilds the item stream of a page after :hover/:active/:focus state changed (see `css::set_state`).
pub fn restyled(src: &Restyle) -> Page { build(&src.dom, &src.base, src.imgs.clone()) }

pub fn build(dom: &Dom, base: &str, imgs: HashMap<usize, Img>) -> Page {
    // layout never sees comment nodes (they only exist for scripts)
    let stripped;
    let dom = if dom.has_comments() { let mut d = dom.clone(); d.strip_comments(); stripped = d; &stripped } else { dom };
    let sheet = Sheet::new(&sheets_of(dom));
    let keep = sheet.has_state.then(|| Arc::new(Restyle { dom: dom.clone(), imgs: imgs.clone(), base: base.to_string() }));
    let mut cx = Ctx { dom, sheet, base, items: vec![], links: vec![], link: None, pending_space: false, bg: None, bg_stack: vec![], imgs, images: vec![], forms: vec![], fields: vec![], cur_form: None, indent: 0.0, node: 0, cb_h: Some(720.0), counters: vec![], glob: HashMap::new() };
    if let Some(&html) = dom.nodes[0].kids.first() { walk(&mut cx, html, &Style::root(), false); }
    let title = dom.find("title").map(|t| dom.text_content(t).split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
    Page { items: cx.items, links: cx.links, title, bg: cx.bg.unwrap_or(0xFFFFFF), images: cx.images, forms: cx.forms, fields: cx.fields, faces: cx.sheet.faces.clone(), webfonts: vec![], parents: dom.nodes.iter().map(|n| n.parent).collect(), body: dom.find("body").unwrap_or(0), live: None, restyle: keep }
}

/// Registers a form control and (unless hidden) places it inline.
fn field(cx: &mut Ctx, id: usize, s: &Style) {
    let dom = cx.dom;
    let a = |n: &str| dom.attr(id, n).unwrap_or("").to_string();
    let ty = a("type").to_ascii_lowercase();
    let tag = dom.nodes[id].tag.as_str();
    let (kind, label) = if tag == "button" {
        (FieldKind::Submit, dom.text_content(id).split_whitespace().collect::<Vec<_>>().join(" "))
    } else if tag == "select" {
        (FieldKind::Select, String::new())
    } else if tag == "textarea" {
        (FieldKind::TextArea, String::new())
    } else {
        match ty.as_str() {
            "hidden" => (FieldKind::Hidden, String::new()),
            "password" => (FieldKind::Password, String::new()),
            "submit" | "button" | "reset" | "image" => (FieldKind::Submit, a("value")),
            "checkbox" => (FieldKind::Check, String::new()),
            "radio" => (FieldKind::Radio, String::new()),
            "file" | "range" | "color" | "date" | "datetime-local" | "time" | "month" | "week" => return,
            _ => (FieldKind::Text, String::new()),
        }
    };
    // Plain buttons and resets do nothing without scripting, so they are not tied to the form.
    let inert = matches!(ty.as_str(), "button" | "reset") && tag != "button" || tag == "button" && ty == "button" || ty == "reset";
    // icon-only buttons (no text) cannot be drawn meaningfully, so they are skipped
    if tag == "button" && label.is_empty() { return; }
    let label = if kind == FieldKind::Submit && label.is_empty() { "Submit".into() } else { label };
    let mut options = vec![];
    let mut sel = 0;
    if kind == FieldKind::Select {
        let mut st = vec![id];
        let mut order = vec![];
        while let Some(i) = st.pop() { order.push(i); st.extend(dom.nodes[i].kids.iter().rev()); }
        for o in order.into_iter().filter(|&o| dom.nodes[o].tag == "option") {
            let text = dom.text_content(o).split_whitespace().collect::<Vec<_>>().join(" ");
            if dom.attr(o, "selected").is_some() { sel = options.len(); }
            options.push((dom.attr(o, "value").map_or_else(|| text.clone(), String::from), text));
        }
    }
    let value = match kind {
        FieldKind::Submit if tag != "button" && ty != "image" => String::new(),
        FieldKind::TextArea => dom.text_content(id).strip_prefix('\n').map_or_else(|| dom.text_content(id), String::from),
        FieldKind::Select => options.get(sel).map(|o| o.0.clone()).unwrap_or_default(),
        _ => a("value"),
    };
    let max_opt = options.iter().map(|o| o.1.chars().count()).max().unwrap_or(4) as f32;
    cx.fields.push(Field {
        form: cx.cur_form.filter(|_| !inert), name: a("name"), initial: value.clone(), value, label: label.clone(), placeholder: a("placeholder"), kind,
        checked: dom.attr(id, "checked").is_some(), fs: s.size, node: id, initial_checked: dom.attr(id, "checked").is_some(), options, sel,
    });
    let idx = cx.fields.len() - 1;
    let size = s.size;
    let (w, h) = match kind {
        FieldKind::Hidden => return,
        FieldKind::Check | FieldKind::Radio => (size * 1.1, size * 1.1),
        FieldKind::Select => ((max_opt * size * 0.55 + 40.0).clamp(80.0, 400.0), size * 1.6),
        FieldKind::TextArea => {
            let n = |k: &str, d: f32| dom.attr(id, k).and_then(|v| v.parse::<f32>().ok()).filter(|&c| c > 0.0).unwrap_or(d);
            ((n("cols", 20.0) * size * 0.55 + 12.0).clamp(100.0, 700.0), n("rows", 3.0).min(20.0) * size * 1.35 + 10.0)
        }
        FieldKind::Submit => (label.chars().count() as f32 * size * 0.58 + 24.0, size * 1.6),
        _ => {
            let chars = dom.attr(id, "size").and_then(|v| v.parse::<f32>().ok()).filter(|&c| c > 0.0).unwrap_or(20.0);
            ((chars * size * 0.55 + 12.0).clamp(60.0, 500.0), size * 1.6)
        }
    };
    cx.items.push(Item::Field { idx, w, h, indent: 0.0, align: s.align, node: id });
    cx.pending_space = true;
}

fn word(cx: &mut Ctx, text: &str, s: &Style, space: bool) {
    let text = &match s.tt {
        1 => text.to_uppercase(),
        2 => text.to_lowercase(),
        3 => { let mut c = text.chars(); c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default() }
        _ => text.to_string(),
    };
    // Text must stay readable on whatever box it sits in (our backgrounds and the author's colors may disagree).
    let bg = cx.bg_stack.last().copied().or(cx.bg).unwrap_or(0xFFFFFF);
    let mut color = if (lum(s.color) - lum(bg)).abs() < 60.0 { if lum(bg) > 128.0 { 0x111111 } else { 0xEEEEEE } } else { s.color };
    if s.alpha < 1.0 { color = mix(color, bg, s.alpha); }
    cx.items.push(Item::Word { text: text.into(), size: s.size, color, bold: s.bold, italic: s.italic, fam: s.fam, ul: s.underline, mono: s.mono, link: cx.link, space, indent: std::mem::take(&mut cx.indent), align: s.align, lh: s.lh, node: cx.node, ls: s.ls, dy: s.x.va });
}

/// Splits a space-free run at the break opportunities of UAX #14 (after hyphens and slashes, between CJK characters
/// with the kinsoku rules for closing and opening punctuation, after soft hyphens); each piece is laid out as its own word.
fn split_cjk(w: &str) -> Vec<&str> {
    if w.bytes().all(|b| b.is_ascii_alphanumeric()) { return vec![w]; }
    let (mut out, mut start) = (vec![], 0);
    for (at, op) in unicode_linebreak::linebreaks(w) {
        if at < w.len() && op == unicode_linebreak::BreakOpportunity::Allowed && at > start { out.push(&w[start..at]); start = at; }
    }
    if start < w.len() { out.push(&w[start..]); }
    out
}

fn len_attr(v: Option<&str>) -> Option<Len> {
    let v = v?.trim();
    match v.strip_suffix('%') {
        Some(p) => p.trim().parse().ok().map(Len::Pct),
        None => v.trim_end_matches("px").trim().parse().ok().map(Len::Px),
    }
}

fn collect_rows(dom: &Dom, id: usize, out: &mut Vec<usize>) {
    for &k in &dom.nodes[id].kids {
        match dom.nodes[k].tag.as_str() {
            "tr" | "caption" => out.push(k),
            "thead" | "tbody" | "tfoot" => collect_rows(dom, k, out),
            _ => {}
        }
    }
}

/// Builds a Table item: every cell's content is walked into its own item list.
fn table(cx: &mut Ctx, id: usize, s: &Style, parent: &Style) {
    let dom = cx.dom;
    let num = |n: &str, d: f32| dom.attr(id, n).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(d);
    let cellpad = num("cellpadding", 1.0).clamp(0.0, 40.0);
    let spacing = if s.collapse { 0.0 } else { num("cellspacing", 2.0).clamp(0.0, 40.0) };
    let border = dom.attr(id, "border").map_or(0.0, |v| v.trim().parse::<f32>().unwrap_or(1.0));
    let width = s.width.or_else(|| len_attr(dom.attr(id, "width")));
    let align = match dom.attr(id, "align").map(str::to_ascii_lowercase).as_deref() { Some("center") => 1, Some("right") => 2, Some("left") => 0, _ => parent.align };
    if let Some(b) = s.bg { cx.bg_stack.push(b); }
    let mut row_ids = vec![];
    collect_rows(dom, id, &mut row_ids);
    let mut rows = vec![];
    for r in row_ids {
        let rs = css::compute(dom, &cx.sheet, r, s);
        if rs.none { continue; }
        let mut cells = vec![];
        let cell_nodes: Vec<usize> = if dom.nodes[r].tag == "caption" { vec![r] } else { dom.nodes[r].kids.iter().copied().filter(|&c| matches!(dom.nodes[c].tag.as_str(), "td" | "th")).collect() };
        for c in cell_nodes {
            let cs = if c == r { rs.clone() } else { css::compute(dom, &cx.sheet, c, &rs) };
            if cs.none { continue; }
            let eff = cs.bg.or(rs.bg);
            if let Some(b) = eff { cx.bg_stack.push(b); }
            let saved = std::mem::take(&mut cx.items);
            cx.pending_space = false;
            for &k in &dom.nodes[c].kids { walk(cx, k, &cs, false); }
            let items = std::mem::replace(&mut cx.items, saved);
            cx.pending_space = false;
            if eff.is_some() { cx.bg_stack.pop(); }
            let valign = match dom.attr(c, "valign").map(str::to_ascii_lowercase).as_deref() { Some("middle") => 1, Some("bottom") => 2, Some("top") => 0, _ => cs.valign };
            cells.push(Cell {
                items,
                colspan: if c == r { 1000 } else { dom.attr(c, "colspan").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 64) },
                width: cs.width.or_else(|| len_attr(dom.attr(c, "width"))),
                bg: cs.bg,
                pad: if cs.pad_set { cs.pad } else { [cellpad; 4] },
                valign,
                nowrap: dom.attr(c, "nowrap").is_some(),
                rowspan: if c == r { 1 } else { dom.attr(c, "rowspan").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 64) },
            });
        }
        rows.push(Row { cells, bg: rs.bg });
    }
    if s.bg.is_some() { cx.bg_stack.pop(); }
    cx.items.push(Item::Break(s.mt));
    cx.items.push(Item::Table(Box::new(Table { rows, width, border, spacing, bg: s.bg, align })));
    cx.items.push(Item::Break(s.mb));
    cx.pending_space = false;
}

fn px_attr(dom: &Dom, id: usize, name: &str) -> Option<f32> { dom.attr(id, name)?.trim().trim_end_matches("px").parse().ok().filter(|&v: &f32| v > 0.0) }

fn walk(cx: &mut Ctx, id: usize, parent: &Style, forced_inline: bool) {
    let dom = cx.dom;
    cx.node = id;
    let n = &dom.nodes[id];
    if n.tag.is_empty() {
        if parent.pre {
            // preformatted: every source line is one unwrapped run with its spaces kept
            let text = n.text.replace('\t', "    ");
            let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
            for (i, l) in lines.iter().enumerate() {
                if i > 0 { cx.items.push(Item::Break(0.0)); }
                if !l.is_empty() { word(cx, l, parent, false); }
            }
            return;
        }
        if n.text.trim().is_empty() { cx.pending_space |= !n.text.is_empty(); return; }
        let mut space = cx.pending_space || n.text.starts_with(char::is_whitespace);
        if parent.nowrap {
            // nowrap: the whole text node is one unbreakable word
            word(cx, &n.text.split_whitespace().collect::<Vec<_>>().join(" "), parent, space);
        } else {
            for w in n.text.split_whitespace() {
                for (k, piece) in split_cjk(w).into_iter().enumerate() { word(cx, piece, parent, space && k == 0); }
                space = true;
            }
        }
        cx.pending_space = n.text.ends_with(char::is_whitespace);
        return;
    }
    let s = css::compute(dom, &cx.sheet, id, parent);
    if s.none { return; }
    if s.clear != 0 { cx.items.push(Item::Clear(s.clear)); }
    if (s.float != 0 || s.x.inline_block || s.x.pos >= 2) && !forced_inline {
        // Render the element normally into its own item list, then hand it to the layout as a float / inline-block / absolute box.
        let mut inner = s.clone();
        let (float, inline_block, abs) = (s.float, s.x.inline_block, s.x.pos >= 2);
        inner.float = 0;
        inner.clear = 0;
        inner.block = true;
        inner.x.pos = 0;
        inner.x.inline_block = false;
        let (width, max_width) = (s.width, s.max_width);
        inner.width = width.map(|_| Len::Pct(100.0));
        inner.max_width = None;
        let saved = std::mem::take(&mut cx.items);
        cx.pending_space = false;
        walk_styled(cx, id, &inner, parent, false);
        let items = std::mem::replace(&mut cx.items, saved);
        cx.pending_space = !abs && inline_block && float == 0;
        cx.items.push(if abs { Item::Abs(Box::new(AbsSpec { items, off: s.x.off, width, max_width, fixed: s.x.pos == 3, z: s.x.z })) }
            else if float == 0 { Item::InlineBlock(Box::new(FloatSpec { right: false, width, max_width, items })) }
            else { Item::Float(Box::new(FloatSpec { right: float == 2, width, max_width, items })) });
        return;
    }
    walk_styled(cx, id, &s, parent, forced_inline);
}

/// Control a `<label>` stands for: `for=id`, else the first control inside it.
fn label_target(dom: &Dom, id: usize) -> Option<usize> {
    if let Some(f) = dom.attr(id, "for") { return dom.descendants().into_iter().find(|&i| dom.attr(i, "id") == Some(f)); }
    let mut st: Vec<usize> = dom.nodes[id].kids.iter().rev().copied().collect();
    while let Some(i) = st.pop() {
        if matches!(dom.nodes[i].tag.as_str(), "input" | "select" | "textarea") { return Some(i); }
        st.extend(dom.nodes[i].kids.iter().rev());
    }
    None
}

/// Inline text of a `::before` (1) / `::after` (2) pseudo-element with `content: "..."`.
fn pseudo_text(cx: &mut Ctx, id: usize, s: &Style, which: u8) {
    let Some((ps, text)) = css::pseudo(cx.dom, &cx.sheet, id, which, s) else { return };
    if ps.none { return; }
    cx.node = id;
    let text = resolve_counters(cx, &text);
    let mut space = cx.pending_space || text.starts_with(char::is_whitespace);
    for w in text.split_whitespace() { word(cx, w, &ps, space); space = true; }
    cx.pending_space = text.ends_with(char::is_whitespace);
}

/// Replaces the `\u{1}name\u{2}sep\u{3}` markers of a `content` value with the counters' current values.
fn resolve_counters(cx: &Ctx, text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(a) = rest.find('\u{1}') {
        out.push_str(&rest[..a]);
        let Some(b) = rest[a..].find('\u{3}').map(|b| a + b) else { break };
        let body = &rest[a + 1..b];
        let (name, sep) = body.split_once('\u{2}').unwrap_or((body, ""));
        let vals: Vec<i32> = cx.counters.iter().filter(|c| c.0 == name).map(|c| c.1).collect();
        let vals = if vals.is_empty() { vec![cx.glob.get(name).copied().unwrap_or(0)] } else { vals };
        if sep.is_empty() { out.push_str(&vals.last().copied().unwrap_or(0).to_string()); } else { out.push_str(&vals.iter().map(i32::to_string).collect::<Vec<_>>().join(sep)); }
        rest = &rest[b + 1..];
    }
    out.push_str(rest);
    out
}

/// Row, column or grid container: every element child (and every run of text) becomes a flex item with its own item list.
fn flex(cx: &mut Ctx, id: usize, s: &Style, mode: u8) {
    let dom = cx.dom;
    let mut items = vec![];
    let mut run: Vec<usize> = vec![]; // consecutive text nodes form one anonymous item
    macro_rules! end_run { () => { if !run.is_empty() {
        let saved = std::mem::take(&mut cx.items);
        cx.pending_space = false;
        for &k in &run { walk(cx, k, s, false); }
        let list = std::mem::replace(&mut cx.items, saved);
        if !list.is_empty() { items.push(FlexItem { items: list, grow: 0.0, shrink: 1.0, basis: None, width: None, max_width: None, auto_l: false, auto_r: false, align: 0, span: 1 }); }
        run.clear();
    } }; }
    for &k in &dom.nodes[id].kids {
        if dom.nodes[k].tag.is_empty() { if !dom.nodes[k].text.trim().is_empty() { run.push(k); } continue; }
        end_run!();
        let cs = css::compute(dom, &cx.sheet, k, s);
        if cs.none { continue; }
        let mut inner = cs.clone();
        (inner.float, inner.clear, inner.block, inner.x.inline_block) = (0, 0, true, false);
        inner.width = cs.width.map(|_| Len::Pct(100.0));
        (inner.max_width, inner.auto_l, inner.auto_r) = (None, false, false);
        if cs.x.pos >= 2 { // absolute and fixed children leave the flex flow
            let saved = std::mem::take(&mut cx.items);
            inner.x.pos = 0;
            walk_styled(cx, k, &inner, s, false);
            let list = std::mem::replace(&mut cx.items, saved);
            cx.items.push(Item::Abs(Box::new(AbsSpec { items: list, off: cs.x.off, width: cs.width, max_width: cs.max_width, fixed: cs.x.pos == 3, z: cs.x.z })));
            continue;
        }
        let saved = std::mem::take(&mut cx.items);
        cx.pending_space = false;
        walk_styled(cx, k, &inner, s, false);
        let list = std::mem::replace(&mut cx.items, saved);
        cx.pending_space = false;
        items.push(FlexItem { items: list, grow: cs.x.grow, shrink: cs.x.shrink, basis: cs.x.basis, width: cs.width, max_width: cs.max_width, auto_l: cs.auto_l, auto_r: cs.auto_r, align: cs.x.align, span: cs.x.span.max(1) });
    }
    end_run!();
    cx.items.push(Item::Break(0.0));
    cx.items.push(if mode == 2 {
        Item::Grid(Box::new(GridSpec { items, tracks: s.x.tracks.clone(), auto_min: s.x.auto_min, col_gap: s.x.gap, row_gap: s.x.row_gap.unwrap_or(s.x.gap) }))
    } else {
        Item::Flex(Box::new(FlexSpec { items, wrap: s.x.wrap, gap: s.x.gap, justify: s.x.justify, align: s.x.align, col: mode == 1 }))
    });
    cx.pending_space = false;
}

fn walk_styled(cx: &mut Ctx, id: usize, s: &Style, parent: &Style, forced_inline: bool) {
    let dom = cx.dom;
    let n = &dom.nodes[id];
    let mut s = s.clone();
    // a percentage height needs a definite containing block; otherwise it is auto
    let mut from_pct = false;
    if let (Some((p, exact)), Some(ph)) = (s.x.h_pct, cx.cb_h) {
        s.x.min_h = s.x.min_h.max((ph * p / 100.0).clamp(0.0, 20000.0));
        s.x.fix_h |= exact;
        from_pct = true;
    }
    match n.tag.as_str() {
        "hr" => {
            cx.items.push(Item::Break(s.mt.max(8.0)));
            cx.items.push(Item::Rule(s.borders[0].map_or(0x999999, |b| b.1)));
            cx.items.push(Item::Break(s.mb.max(8.0)));
            cx.pending_space = false;
            return;
        }
        "br" => { cx.items.push(Item::Break(0.0)); cx.pending_space = false; return; }
        // an <svg> is a picture too; svg content that failed to rasterize (or nested svg parts) is simply skipped
        "img" | "svg" => {
            if let Some(im) = cx.imgs.remove(&id) {
                let (nw, nh) = (im.w as f32, im.h as f32);
                // CSS beats the width/height attributes; one given dimension fixes the other by the picture's proportions
                let css_w = if let Some(Len::Px(p)) = s.width { Some(p) } else { None };
                let pct = if let Some(Len::Pct(p)) = s.width { Some(p) } else { None };
                let css_h = s.x.fix_h.then_some(s.x.min_h).filter(|h| *h > 0.0);
                let (aw, ah) = (css_w.or_else(|| px_attr(dom, id, "width")).filter(|_| css_h.is_none() || css_w.is_some()), css_h.or_else(|| px_attr(dom, id, "height")).filter(|_| css_w.is_none() && pct.is_none() || css_h.is_some()));
                let (mut w, mut h) = match (aw, ah) {
                    (Some(w), Some(h)) => (w, h),
                    (Some(w), None) => (w, w * nh / nw),
                    (None, Some(h)) => (h * nw / nh, h),
                    // an inline <svg> without any size is 300 px wide at most (browsers use 300 x 150)
                    _ if n.tag == "svg" && pct.is_none() => { let k = (300.0 / nw).min(1.0); (nw * k, nh * k) }
                    _ => (nw, nh),
                };
                if let Some(Len::Px(mw)) = s.max_width { if w > mw { h *= mw / w; w = mw; } }
                let k = (MAX_IMG_CSS_W / w).min(1.0);
                cx.images.push(im);
                cx.items.push(Item::Image { img: cx.images.len() - 1, w: w * k, h: h * k, link: cx.link, indent: 0.0, align: s.align, node: id, va: s.x.va_kind, alpha: s.alpha, pct });
                cx.pending_space = true;
            } else if let Some(alt) = dom.attr(id, "alt").filter(|a| !a.trim().is_empty()) {
                word(cx, &format!("[{}]", alt.trim()), &s, true);
            }
            return;
        }
        "input" | "button" | "select" | "textarea" => { field(cx, id, &s); return; }
        "table" => { table(cx, id, &s, parent); return; }
        "html" | "body" => if s.bg.is_some() { cx.bg = s.bg },
        _ => {}
    }
    let saved_form = cx.cur_form;
    if n.tag == "form" {
        let action = resolve(cx.base, dom.attr(id, "action").filter(|a| !a.trim().is_empty()).unwrap_or(cx.base));
        cx.forms.push(Form { action, post: dom.attr(id, "method").is_some_and(|m| m.eq_ignore_ascii_case("post")), node: id });
        cx.cur_form = Some(cx.forms.len() - 1);
    }
    let saved_link = cx.link;
    if n.tag == "label" {
        if let Some(t) = label_target(dom, id) { cx.links.push(format!("label:{t}")); cx.link = Some(cx.links.len() - 1); }
    }
    if n.tag == "a" {
        cx.link = dom.attr(id, "href").map(|h| { cx.links.push(resolve(cx.base, h)); cx.links.len() - 1 });
    }
    let block = s.block && !forced_inline;
    let spaced = forced_inline;
    let bg_box = s.bg.filter(|&b| Some(b) != cx.bg || s.bg_a < 1.0 || s.alpha < 1.0 || s.bg_grad.is_some());
    let bg_a = s.bg_a * s.alpha;
    let counter_mark = cx.counters.len();
    for (name, v) in &s.x.counter_reset { cx.counters.push((name.clone(), *v)); }
    for (name, d) in &s.x.counter_inc {
        match cx.counters.iter_mut().rev().find(|c| c.0 == *name) { Some(c) => c.1 += d, None => *cx.glob.entry(name.clone()).or_insert(0) += d }
    }
    let bgimg = s.bg_img.as_ref().and_then(|b| cx.imgs.remove(&(usize::MAX - id)).map(|im| {
        let (nw, nh) = (im.w as f32, im.h as f32);
        cx.images.push(im);
        Arc::new(BgPaint { img: cx.images.len() - 1, nw, nh, size: b.size, pos: b.pos, repeat: b.repeat })
    }));
    let sized = bgimg.is_some() || s.x.shadow.is_some() || s.ml != 0.0 || s.mr != 0.0 || s.pl != 0.0 || s.pr != 0.0 || s.width.is_some() || s.max_width.is_some() || s.auto_l || s.auto_r
        || s.borders.iter().any(Option::is_some) || s.x.min_h > 0.0;
    let boxed = block && (bg_box.is_some() || sized);
    // a box paints its own vertical padding, so the margin gap before it is only the margin
    if block { cx.items.push(Item::Break(if boxed { s.mt - s.pt } else { s.mt })); cx.pending_space = false; cx.indent = s.x.indent; }
    if boxed {
        cx.items.push(Item::Box(BoxSpec {
            ml: s.ml, mr: s.mr, pl: s.pl, pr: s.pr, width: s.width, max_width: s.max_width, auto_l: s.auto_l, auto_r: s.auto_r, bg: bg_box, bb: s.x.bb, min_h: s.x.min_h, radius: s.x.radius, node: id, borders: s.borders,
            pt: s.pt, pb: s.pb, bgimg: bgimg.clone(), bg_a,
            clip: s.x.clip && !s.x.scroll && s.x.fix_h && !from_pct && !matches!(n.tag.as_str(), "html" | "body"),
            scroll: s.x.scroll && s.x.fix_h && !from_pct && !matches!(n.tag.as_str(), "html" | "body"), shadow: s.x.shadow.map(|sh| Shadow { a: sh.a * s.alpha, ..sh }), grad: s.bg_grad.clone(),
        }));
        if let Some(b) = bg_box { let eff = if bg_a < 1.0 { mix(b, backdrop(cx), bg_a) } else { b }; cx.bg_stack.push(eff); }
    }
    let rel = block && s.x.pos == 1;
    if rel { cx.items.push(Item::RelStart(s.x.off, s.x.z, s.x.sticky)); }
    if spaced { cx.pending_space = true; }
    if s.list_item && block && s.bullets {
        let marker = match dom.nodes.get(n.parent).filter(|p| p.tag == "ol") {
            Some(ol) => {
                let start = dom.attr(n.parent, "start").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(1);
                let k = ol.kids.iter().take_while(|&&k| k != id).filter(|&&k| dom.nodes[k].tag == "li").count() as i64;
                format!("{}.", dom.attr(id, "value").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(start + k))
            }
            None => "•".to_string(),
        };
        word(cx, &marker, &s, false);
        cx.pending_space = true;
    }
    pseudo_text(cx, id, &s, 1);
    let saved_cb = if block { Some(std::mem::replace(&mut cx.cb_h, s.x.fix_h.then_some(s.x.min_h))) } else { None };
    if block && (s.flex_row || s.x.flex_col || s.x.grid) { flex(cx, id, &s, if s.flex_row { 0 } else if s.x.flex_col { 1 } else { 2 }); } else { for &k in &n.kids { walk(cx, k, &s, s.flex_row); } }
    pseudo_text(cx, id, &s, 2);
    if boxed { cx.items.push(Item::BoxEnd); if bg_box.is_some() { cx.bg_stack.pop(); } }
    if rel { cx.items.push(Item::RelEnd); } // after BoxEnd: the box's own min-height, background and borders move with it
    if block { cx.items.push(Item::Break(if boxed { s.mb - s.pb } else { s.mb })); cx.pending_space = false; }
    if spaced && !block { cx.pending_space = true; }
    if let Some(c) = saved_cb { cx.cb_h = c; }
    cx.counters.truncate(counter_mark);
    cx.link = saved_link;
    cx.cur_form = saved_form;
}

pub fn resolve(base: &str, h: &str) -> String { dedot(resolve_raw(base, h)) }

/// Removes `.` and `..` segments from the path of an absolute URL.
fn dedot(url: String) -> String {
    let Some(s) = url.find("://") else { return url };
    let Some(p) = url[s + 3..].find('/') else { return url };
    let start = s + 3 + p;
    let tail_at = url[start..].find(['?', '#']).map_or(url.len(), |i| start + i);
    let (path, tail) = (&url[start..tail_at], &url[tail_at..]);
    if !path.contains("/.") { return url; }
    let mut out: Vec<&str> = vec![];
    for seg in path.split('/').skip(1) { match seg { "." => {} ".." => { out.pop(); } x => out.push(x) } }
    let dir = path.ends_with("/.") || path.ends_with("/..");
    format!("{}/{}{}{}", &url[..start], out.join("/"), if dir && !out.is_empty() { "/" } else { "" }, tail)
}

fn resolve_raw(base: &str, h: &str) -> String {
    if h.contains("://") || h.starts_with('#') || h.contains(':') && !h.contains('/') { return h.into(); }
    let base = base.split(['?', '#']).next().unwrap_or(base);
    let Some(s) = base.find("://") else { return h.into() };
    let origin_end = base[s + 3..].find('/').map_or(base.len(), |p| p + s + 3);
    if let Some(rest) = h.strip_prefix("//") { return format!("{}://{}", &base[..s], rest); }
    if h.starts_with('/') { return format!("{}{}", &base[..origin_end], h); }
    match base[origin_end..].rfind('/') {
        Some(p) => format!("{}{}", &base[..origin_end + p + 1], h),
        None => format!("{}/{}", base, h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::parse;

    fn words(items: &[Item]) -> String {
        items.iter().map(|i| match i {
            Item::Word { text, space, .. } => format!("{}{}", if *space { " " } else { "" }, text),
            Item::Break(_) => "|".into(),
            _ => String::new(),
        }).collect()
    }

    fn build(d: &Dom, base: &str) -> Page { super::build(d, base, HashMap::new()) }

    #[test]
    fn flow_links_lists_hidden() {
        let d = parse("<title>t</title><style>.x{display:none}</style><h1>Hi &amp; bye</h1><p>see <a href=\"/x\">this</a>.<span class=x>no</span></p><ul><li>one</li><li>two <img alt=pic></li></ul>", true);
        let pg = build(&d, "https://a.com/d/e");
        assert_eq!(words(&pg.items), "|||Hi & bye||see this.|||• one||• two [pic]||||");
        assert_eq!(pg.links, vec!["https://a.com/x"]);
        assert_eq!(pg.title, "t");
        assert!(matches!(pg.items[3], Item::Word { size, bold: true, .. } if size == 32.0));
    }

    #[test]
    fn tables_become_rows_and_cells() {
        let d = parse("<table width=80% border=1 cellpadding=3><thead><tr><th>H</th></tr></thead><tr><td>1.</td><td colspan=2 align=right><a href=x>Title</a></td></tr></table>", true);
        let pg = build(&d, "https://a.com");
        let Some(Item::Table(t)) = pg.items.iter().find(|i| matches!(i, Item::Table(_))) else { panic!("no table") };
        assert_eq!((t.rows.len(), t.rows[1].cells.len(), t.border, t.width), (2, 2, 1.0, Some(Len::Pct(80.0))));
        assert_eq!((t.rows[1].cells[1].colspan, t.rows[1].cells[0].pad), (2, [3.0; 4]));
        assert!(matches!(t.rows[1].cells[1].items[0], Item::Word { align: 2, link: Some(0), .. }));
        assert!(matches!(t.rows[0].cells[0].items[0], Item::Word { bold: true, align: 1, .. }));
    }

    #[test]
    fn images_and_boxes() {
        let d = parse("<div style='background:#ff0000;color:#fff'>hi</div><a href=/x><img src=a.png width=50></a>", true);
        let id = d.find("img").unwrap();
        let mut imgs = HashMap::new();
        imgs.insert(id, Img { w: 100, h: 40, px: vec![0; 100 * 40 * 4] });
        let pg = super::build(&d, "https://a.com", imgs);
        assert_eq!(pg.images.len(), 1);
        assert!(pg.items.iter().any(|i| matches!(i, Item::Box(b) if b.bg == Some(0xFF0000))));
        assert!(pg.items.iter().any(|i| matches!(i, Item::Image { w, h, link: Some(0), .. } if *w == 50.0 && *h == 20.0)));
        assert!(pg.items.iter().any(|i| matches!(i, Item::Word { color: 0xFFFFFF, .. })));
    }

    #[test]
    fn forms_and_fields() {
        let d = parse("<form action=/s method=get><input type=hidden name=h value=1><input name=q value=hi size=10><button>Go</button><input type=button value=No></form><input name=out>", true);
        let pg = build(&d, "https://a.com/x/y");
        assert_eq!(pg.forms.len(), 1);
        assert_eq!((pg.forms[0].action.as_str(), pg.forms[0].post), ("https://a.com/s", false));
        let f: Vec<_> = pg.fields.iter().map(|f| (f.name.as_str(), f.form, f.kind)).collect();
        assert_eq!(f, vec![("h", Some(0), FieldKind::Hidden), ("q", Some(0), FieldKind::Text), ("", Some(0), FieldKind::Submit), ("", None, FieldKind::Submit), ("out", None, FieldKind::Text)]);
        assert_eq!(pg.fields[2].label, "Go");
        assert_eq!(pg.items.iter().filter(|i| matches!(i, Item::Field { .. })).count(), 4); // hidden is not drawn
    }

    #[test]
    fn select_textarea_radio() {
        let d = parse("<form><select name=c><option value=a>Alpha<option value=b selected>Beta</select><textarea name=m rows=4>\nhello</textarea><input type=radio name=r value=1 checked><input type=radio name=r value=2></form>", true);
        let pg = build(&d, "https://a.com");
        let f = &pg.fields;
        assert_eq!((f[0].kind, f[0].sel, f[0].value.as_str(), f[0].options.len()), (FieldKind::Select, 1, "b", 2));
        assert_eq!((f[1].kind, f[1].value.as_str()), (FieldKind::TextArea, "hello"));
        assert_eq!((f[2].kind, f[2].checked, f[3].checked), (FieldKind::Radio, true, false));
    }

    #[test]
    fn page_background_and_contrast() {
        let d = parse("<body style='background:#000'><p style='color:#111'>x</p>", true);
        let pg = build(&d, "https://a.com");
        assert_eq!(pg.bg, 0);
        assert!(matches!(pg.items.iter().find(|i| matches!(i, Item::Word { .. })), Some(Item::Word { color: 0xEEEEEE, .. })));
    }

    #[test]
    fn resolve_cases() {
        assert_eq!(resolve("https://a.com/d/e?q=1", "f"), "https://a.com/d/f");
        assert_eq!(resolve("https://a.com", "f"), "https://a.com/f");
        assert_eq!(resolve("https://a.com/d/e", "//b.com/x"), "https://b.com/x");
        assert_eq!(resolve("https://a.com/d/e", "https://c.com"), "https://c.com");
        assert_eq!(resolve("https://a.com/a/d/e", "../b/./c?x=../y#f"), "https://a.com/a/b/c?x=../y#f");
        assert_eq!(resolve("https://a.com/a/b/c", "../../../.."), "https://a.com/");
        assert_eq!(resolve("https://a.com/a/b/c", ".."), "https://a.com/a/");
    }

    #[test]
    fn pseudo_content_and_text_transform() {
        let d = parse("<style>a::before{content:'\\2192  '} p:after{content:\"!\"} h1{text-transform:uppercase} b{content:'x'}</style><a href=x>go</a><p>t</p><h1>big</h1>", true);
        let w = words(&build(&d, "https://a.com").items);
        assert!(w.contains("\u{2192} go") && w.contains("t!") && w.contains("BIG") && !w.contains('x'), "{w}");
    }

    #[test]
    fn labels_point_at_their_controls() {
        let d = parse("<label for=q>Name</label><input id=q name=q><label>Agree <input type=checkbox name=c></label>", true);
        let pg = build(&d, "https://a.com");
        let q = pg.fields.iter().find(|f| f.name == "q").unwrap().node;
        let c = pg.fields.iter().find(|f| f.name == "c").unwrap().node;
        assert_eq!(pg.links, vec![format!("label:{q}"), format!("label:{c}")]);
    }

    #[test]
    fn cjk_runs_split_into_breakable_pieces() {
        assert_eq!(split_cjk("abc"), vec!["abc"]);
        assert_eq!(split_cjk("ab\u{4f60}\u{597d}cd"), vec!["ab", "\u{4f60}", "\u{597d}", "cd"]);
        let d = parse("<p>\u{4f60}\u{597d}\u{4e16}\u{754c}</p>", true);
        assert_eq!(pg_words(&build(&d, "https://a.com")), 4);
    }

    #[test]
    fn css_sizes_override_image_attributes_and_keep_proportions() {
        let size = |html: &str| {
            let d = parse(html, true);
            let (tag, id) = (if d.find("img").is_some() { "img" } else { "svg" }, d.find("img").or_else(|| d.find("svg")).unwrap());
            let _ = tag;
            let pg = super::build(&d, "https://a.com", HashMap::from([(id, Img { w: 100, h: 40, px: vec![0; 100 * 40 * 4] })]));
            pg.items.iter().find_map(|i| if let Item::Image { w, h, pct, .. } = i { Some((*w, *h, *pct)) } else { None }).unwrap()
        };
        assert_eq!(size("<img width=300 height=300>"), (300.0, 300.0, None), "attributes alone");
        assert_eq!(size("<img>"), (100.0, 40.0, None), "natural size");
        assert_eq!(size("<img width=300 height=300 style='width:50px'>"), (50.0, 20.0, None), "CSS width wins, height follows the proportions");
        assert_eq!(size("<img width=300 height=300 style='height:20px'>"), (50.0, 20.0, None));
        assert_eq!(size("<img style='width:1em'>"), (16.0, 6.4, None));
        assert_eq!(size("<img style='max-width:60px'>"), (60.0, 24.0, None));
        assert_eq!(size("<img style='width:50%'>"), (100.0, 40.0, Some(50.0)));
        assert_eq!(size("<img style='max-width:200px;width:150px'>"), (150.0, 60.0, None));
        let (w, h, _) = size("<svg viewBox='0 0 100 40'></svg>");
        assert!(w <= 300.0 && (w / h - 2.5).abs() < 0.01, "an unsized svg is capped at 300 wide: {w}");
    }

    #[test]
    fn hover_active_and_focus_restyle_from_the_kept_dom() {
        let d = parse("<style>a{color:#0000ff} a:hover{color:#ff0000} p:active{color:#00aa00} input:focus{background:#ffff00}</style><p><a href=x>go</a> tail</p><input name=q>", true);
        let (a, p, inp) = (d.find("a").unwrap(), d.find("p").unwrap(), d.find("input").unwrap());
        let color_of = |pg: &Page, word: &str| pg.items.iter().find_map(|i| match i { Item::Word { text, color, .. } if text == word => Some(*color), _ => None }).unwrap();
        css::set_state(0, 0, 0);
        let pg = build(&d, "https://a.com");
        assert!(pg.restyle.is_some(), "pages with state selectors keep what a restyle needs");
        assert_eq!((color_of(&pg, "go"), color_of(&pg, "tail")), (0x0000FF, 0x111111));
        css::set_state(d.nodes[a].kids[0], 0, 0); // the pointer is over the link's text: the link and its ancestors are :hover
        let pg = restyled(pg.restyle.as_ref().unwrap());
        assert_eq!(color_of(&pg, "go"), 0xFF0000);
        css::set_state(0, p, 0);
        let pg = restyled(pg.restyle.as_ref().unwrap());
        assert_eq!((color_of(&pg, "tail"), color_of(&pg, "go")), (0x00AA00, 0x0000FF));
        css::set_state(0, 0, inp);
        let pg = restyled(pg.restyle.as_ref().unwrap());
        assert!(pg.items.iter().any(|i| matches!(i, Item::Box(b) if b.bg == Some(0xFFFF00))) || pg.items.iter().any(|i| matches!(i, Item::Field { .. })), "focus state reaches the control");
        css::set_state(0, 0, 0);
        assert!(build(&parse("<p>plain</p>", true), "https://a.com").restyle.is_none(), "no state selectors, nothing kept");
    }

    #[test]
    fn background_urls_are_found_from_stylesheets_and_inline_styles() {
        let d = parse("<style>.hero{background:url(img/h.png) no-repeat} .x{background-image:none}</style><div class=hero>a</div><div style=\"background-image:url('/b.jpg')\">b</div><div class=x>c</div><p>d</p>", true);
        let urls: Vec<String> = background_urls(&d, "https://a.com/p/q.html").into_iter().map(|u| u.1).collect();
        assert_eq!(urls, vec!["https://a.com/p/img/h.png", "https://a.com/b.jpg"]);
        assert!(background_urls(&parse("<p>none</p>", true), "https://a.com").is_empty());
    }

    #[test]
    fn uax14_breaks_after_hyphens_and_slashes_and_obeys_kinsoku() {
        assert_eq!(split_cjk("well-known"), vec!["well-", "known"]);
        assert_eq!(split_cjk("https://a.com/b/c"), vec!["https://", "a.com/", "b/", "c"]);
        assert_eq!(split_cjk("plain"), vec!["plain"]);
        assert_eq!(split_cjk("co\u{ad}operate"), vec!["co\u{ad}", "operate"]);
        // no line may start with a closing mark: the full stop and the closing bracket stay with the character before them
        assert_eq!(split_cjk("\u{4f60}\u{597d}\u{3002}\u{4e16}"), vec!["\u{4f60}", "\u{597d}\u{3002}", "\u{4e16}"]);
        assert_eq!(split_cjk("\u{4f60}\u{300d}\u{597d}"), vec!["\u{4f60}\u{300d}", "\u{597d}"]);
        // no line may end with an opening bracket
        assert_eq!(split_cjk("\u{4f60}\u{300c}\u{597d}"), vec!["\u{4f60}", "\u{300c}\u{597d}"]);
    }

    fn pg_words(pg: &Page) -> usize { pg.items.iter().filter(|i| matches!(i, Item::Word { .. })).count() }
}
