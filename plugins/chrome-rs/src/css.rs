// CSS: parser, selector matching, cascade for the properties a text-flow renderer can use.
// Supported: display, color, font-size, font-weight, text-align, text-decoration, margin/padding (vertical gaps +
// left indent), background (page only), visibility/opacity:0 (hide), list-style:none, flex-direction.
// shortcut: sticky acts as relative, grid has columns only, no :hover/:focus/:active; @media evaluated against a fixed 1000px viewport.
use crate::dom::Dom;
use std::{borrow::Cow, collections::HashMap, rc::Rc, sync::{atomic::{AtomicUsize, Ordering}, Arc}};

const VIEWPORT_W: f32 = 1000.0;

#[derive(Clone, Copy, PartialEq)]
enum Comb { Desc, Child, Adj, Sib }

struct AttrSel { name: String, op: char, val: String } // op ' ' = exists

#[derive(Default)]
struct Compound { tag: Option<String>, id: Option<String>, classes: Vec<String>, attrs: Vec<AttrSel>, first: bool, last: bool, never: bool, nots: Vec<Compound>,
    nths: Vec<(bool, bool, i32, i32)>, // (of-type, from-last, a, b) for :nth-*(an+b)
    is: Vec<(bool, Vec<Compound>)>,    // :is()/:where() any-of lists (bool = counts toward specificity)
    empty: bool, pseudo: u8,
    state: u8, // :hover 1, :active 2, :focus 4, :focus-within 8
}

struct Complex { parts: Vec<(Comb, Compound)> }

pub struct Decl { prop: String, val: String, imp: bool }

struct Rule { sel: Complex, spec: u32, order: usize, decls: Rc<Vec<Decl>> }

#[derive(Default)]
pub struct Sheet {
    rules: Vec<Rule>,
    by_id: HashMap<String, Vec<usize>>,
    by_class: HashMap<String, Vec<usize>>,
    by_tag: HashMap<String, Vec<usize>>,
    other: Vec<usize>,
    has_pseudo: bool,
    /// some rule depends on :hover/:active/:focus, so the page must be restyled when that state changes
    pub has_state: bool,
    pub faces: Vec<FontFaceRule>,
}

/// One `@font-face` declaration: family (lowercase), weight/style flags and candidate sources in page order.
#[derive(Clone, Debug, PartialEq)]
pub struct FontFaceRule { pub family: String, pub bold: bool, pub italic: bool, pub srcs: Vec<(String, String)> }

fn is_ident(c: char) -> bool { c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii() }

fn parse_complex(s: &str) -> Option<Complex> {
    let cs: Vec<char> = s.trim().chars().collect();
    let (mut parts, mut comb, mut cur, mut have, mut i) = (vec![], Comb::Desc, Compound::default(), false, 0);
    let ident = |i: &mut usize| -> String {
        let s0 = *i;
        while *i < cs.len() && is_ident(cs[*i]) { *i += 1; }
        cs[s0..*i].iter().collect()
    };
    while i < cs.len() {
        match cs[i] {
            c if c.is_whitespace() || matches!(c, '>' | '+' | '~') => {
                let mut nc = None;
                while i < cs.len() && (cs[i].is_whitespace() || matches!(cs[i], '>' | '+' | '~')) {
                    match cs[i] { '>' => nc = Some(Comb::Child), '+' => nc = Some(Comb::Adj), '~' => nc = Some(Comb::Sib), _ => {} }
                    i += 1;
                }
                if have { parts.push((comb, std::mem::take(&mut cur))); have = false; }
                comb = nc.unwrap_or(Comb::Desc);
            }
            '#' => { i += 1; cur.id = Some(ident(&mut i)); have = true; }
            '.' => { i += 1; cur.classes.push(ident(&mut i)); have = true; }
            '*' => { i += 1; have = true; }
            '[' => {
                let close = cs[i..].iter().position(|&c| c == ']')? + i;
                let body: String = cs[i + 1..close].iter().collect();
                i = close + 1;
                have = true;
                let (name, op, val) = match body.find('=') {
                    Some(p) => {
                        let (n, op) = match body[..p].chars().last() { Some(o @ ('~' | '|' | '^' | '$' | '*')) => (&body[..p - 1], o), _ => (&body[..p], '=') };
                        (n.trim().to_string(), op, body[p + 1..].trim().trim_matches(['"', '\'']).to_string())
                    }
                    None => (body.trim().to_string(), ' ', String::new()),
                };
                cur.attrs.push(AttrSel { name: name.to_ascii_lowercase(), op, val });
            }
            ':' => {
                let double = cs.get(i + 1) == Some(&':');
                i += 1 + double as usize;
                let name = ident(&mut i);
                let mut arg = String::new();
                if cs.get(i) == Some(&'(') {
                    let (mut depth, start) = (0, i + 1);
                    while i < cs.len() {
                        if cs[i] == '(' { depth += 1 } else if cs[i] == ')' { depth -= 1; if depth == 0 { arg = cs[start..i].iter().collect(); i += 1; break; } }
                        i += 1;
                    }
                }
                have = true;
                match (double, name.as_str()) {
                    (false, "not") => match parse_complex(&arg) {
                        Some(c) if c.parts.len() == 1 => cur.nots.push(c.parts.into_iter().next().unwrap().1),
                        _ => cur.never = true,
                    },
                    (false, "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type") => match parse_nth(&arg) {
                        Some((a, b)) => cur.nths.push((name.contains("of-type"), name.contains("last"), a, b)),
                        None => cur.never = true,
                    },
                    (false, "first-of-type") => cur.nths.push((true, false, 0, 1)),
                    (false, "last-of-type") => cur.nths.push((true, true, 0, 1)),
                    (false, "only-child") => { cur.first = true; cur.last = true; }
                    (false, "empty") => cur.empty = true,
                    (false, "checked" | "disabled") => cur.attrs.push(AttrSel { name: name.clone(), op: ' ', val: String::new() }),
                    (false, "is" | "where" | "matches" | "any" | "-webkit-any" | "-moz-any") => {
                        let list: Vec<Compound> = split_top(&arg, ',').into_iter().filter_map(parse_complex).filter(|c| c.parts.len() == 1).filter_map(|c| c.parts.into_iter().next().map(|p| p.1)).collect();
                        if list.is_empty() { cur.never = true } else { cur.is.push((name != "where", list)); }
                    }
                    (_, "before" | "after") => cur.pseudo = if name == "before" { 1 } else { 2 },
                    (false, "first-child") => cur.first = true,
                    (false, "last-child") => cur.last = true,
                    (false, "hover") => cur.state |= 1,
                    (false, "active") => cur.state |= 2,
                    (false, "focus" | "focus-visible") => cur.state |= 4,
                    (false, "focus-within") => cur.state |= 8,
                    (false, "link" | "visited" | "any-link") => {}
                    (false, "root") => cur.tag = Some("html".into()),
                    _ => cur.never = true,
                }
            }
            c if is_ident(c) => { cur.tag = Some(ident(&mut i).to_ascii_lowercase()); have = true; }
            _ => return None,
        }
    }
    if have { parts.push((comb, cur)); }
    if parts.is_empty() { None } else { Some(Complex { parts }) }
}

fn parse_nth(s: &str) -> Option<(i32, i32)> {
    let t: String = s.split(" of ").next().unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    match t.as_str() { "odd" => return Some((2, 1)), "even" => return Some((2, 0)), _ => {} }
    match t.split_once('n') {
        None => t.parse().ok().map(|b| (0, b)),
        Some((a, b)) => Some((match a { "" | "+" => 1, "-" => -1, x => x.parse().ok()? }, if b.is_empty() { 0 } else { b.parse().ok()? })),
    }
}

fn compound_spec(p: &Compound) -> u32 {
    p.id.is_some() as u32 * 10_000 + (p.classes.len() + p.attrs.len() + p.first as usize + p.last as usize + p.nths.len() + p.state.count_ones() as usize) as u32 * 100 + p.tag.is_some() as u32
        + p.is.iter().filter(|l| l.0).map(|l| l.1.iter().map(compound_spec).max().unwrap_or(0)).sum::<u32>()
        + p.nots.iter().map(compound_spec).sum::<u32>() // :not(x) counts as its argument
}

fn specificity(c: &Complex) -> u32 { c.parts.iter().map(|(_, p)| compound_spec(p)).sum() }

fn elem_siblings(dom: &Dom, id: usize) -> Vec<usize> {
    dom.nodes[dom.nodes[id].parent].kids.iter().copied().filter(|&k| !dom.nodes[k].tag.is_empty()).collect()
}

fn match_compound(dom: &Dom, id: usize, c: &Compound) -> bool {
    if c.never { return false; }
    if c.nots.iter().any(|n| match_compound(dom, id, n)) { return false; }
    let n = &dom.nodes[id];
    if n.tag.is_empty() || c.tag.as_ref().is_some_and(|t| *t != n.tag) { return false; }
    if c.id.as_deref().is_some_and(|i| dom.attr(id, "id") != Some(i)) { return false; }
    if !c.classes.is_empty() {
        let cl = dom.attr(id, "class").unwrap_or("");
        if !c.classes.iter().all(|k| cl.split_whitespace().any(|x| x == k)) { return false; }
    }
    for a in &c.attrs {
        let Some(v) = dom.attr(id, &a.name) else { return false };
        let ok = match a.op {
            ' ' => true,
            '=' => v == a.val,
            '~' => v.split_whitespace().any(|x| x == a.val),
            '|' => v == a.val || v.starts_with(&format!("{}-", a.val)),
            '^' => v.starts_with(&a.val),
            '$' => v.ends_with(&a.val),
            _ => v.contains(&a.val),
        };
        if !ok { return false; }
    }
    for &(of_type, from_last, a, b) in &c.nths {
        let sib: Vec<usize> = elem_siblings(dom, id).into_iter().filter(|&k| !of_type || dom.nodes[k].tag == n.tag).collect();
        let Some(pos) = sib.iter().position(|&k| k == id) else { return false };
        let d = (if from_last { sib.len() - pos } else { pos + 1 }) as i32 - b;
        if !(if a == 0 { d == 0 } else { d % a == 0 && d / a >= 0 }) { return false; }
    }
    if c.empty && !n.kids.iter().all(|&k| dom.nodes[k].tag.is_empty() && dom.nodes[k].text.is_empty()) { return false; }
    if c.is.iter().any(|(_, list)| !list.iter().any(|x| match_compound(dom, id, x))) { return false; }
    if c.first || c.last {
        let sib = elem_siblings(dom, id);
        if c.first && sib.first() != Some(&id) || c.last && sib.last() != Some(&id) { return false; }
    }
    if c.state != 0 {
        let (hover, active, focus) = state();
        if c.state & 1 != 0 && !within(dom, id, hover) || c.state & 2 != 0 && !within(dom, id, active) || c.state & 4 != 0 && (focus != id || id == 0) || c.state & 8 != 0 && !within(dom, id, focus) { return false; }
    }
    true
}

// Interaction state for :hover / :active / :focus: DOM node ids of the pointer target, the pressed element and the focused control (0 = none).
// A process-wide value because page builders run on other threads than the UI.
static STATE: [AtomicUsize; 3] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];

pub fn set_state(hover: usize, active: usize, focus: usize) { for (s, v) in STATE.iter().zip([hover, active, focus]) { s.store(v, Ordering::Relaxed); } }

fn state() -> (usize, usize, usize) { (STATE[0].load(Ordering::Relaxed), STATE[1].load(Ordering::Relaxed), STATE[2].load(Ordering::Relaxed)) }

/// `id` is `target` or one of its ancestors (the pointer is over a descendant of a hovered element's box).
fn within(dom: &Dom, id: usize, target: usize) -> bool {
    let mut n = target;
    while n != 0 { if n == id { return true; } n = dom.nodes.get(n).map_or(0, |x| x.parent); }
    false
}

fn match_at(dom: &Dom, id: usize, parts: &[(Comb, Compound)], i: usize) -> bool {
    if !match_compound(dom, id, &parts[i].1) { return false; }
    if i == 0 { return true; }
    let parent = dom.nodes[id].parent;
    match parts[i].0 {
        Comb::Desc => { let mut p = parent; while p != 0 { if match_at(dom, p, parts, i - 1) { return true; } p = dom.nodes[p].parent; } false }
        Comb::Child => parent != 0 && match_at(dom, parent, parts, i - 1),
        Comb::Adj | Comb::Sib => {
            let sib = elem_siblings(dom, id);
            let pos = sib.iter().position(|&s| s == id).unwrap_or(0);
            if parts[i].0 == Comb::Adj { pos > 0 && match_at(dom, sib[pos - 1], parts, i - 1) } else { sib[..pos].iter().any(|&s| match_at(dom, s, parts, i - 1)) }
        }
    }
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(p) = rest.find("/*") {
        out.push_str(&rest[..p]);
        rest = rest[p + 2..].find("*/").map_or("", |e| &rest[p + 2 + e + 2..]);
    }
    out + rest
}

fn matching_brace(css: &str, open: usize) -> usize {
    let (mut depth, b) = (0i32, css.as_bytes());
    for (k, &c) in b.iter().enumerate().skip(open) {
        if c == b'{' { depth += 1 } else if c == b'}' { depth -= 1; if depth == 0 { return k; } }
    }
    b.len()
}

fn media_ok(q: &str) -> bool {
    let q = q.to_ascii_lowercase();
    if ["print", "speech", "prefers-color-scheme: dark", "pointer: coarse", "hover: none", "prefers-reduced-motion: reduce"].iter().any(|k| q.contains(k)) { return false; }
    let num = |key: &str| -> Vec<f32> {
        q.match_indices(key).filter_map(|(p, _)| {
            let r = q[p + key.len()..].trim_start();
            let end = r.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(r.len());
            let n: f32 = r[..end].parse().ok()?;
            Some(if r[end..].starts_with("em") { n * 16.0 } else { n })
        }).collect()
    };
    num("max-width:").iter().all(|&n| n >= VIEWPORT_W) && num("min-width:").iter().all(|&n| n <= VIEWPORT_W)
}

/// Whitespace split that keeps `calc(1px + 2px)` together.
fn split_ws(s: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (vec![], 0i32, None::<usize>);
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c.is_whitespace() && depth <= 0 => { if let Some(st) = start.take() { out.push(&s[st..i]); } continue; }
            _ => {}
        }
        start.get_or_insert(i);
    }
    if let Some(st) = start { out.push(&s[st..]); }
    out
}

fn split_top(s: &str, sep: char) -> Vec<&str> {
    let (mut out, mut depth, mut quote, mut last) = (vec![], 0i32, None::<char>, 0);
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, c) if c == sep && depth == 0 => { out.push(&s[last..i]); last = i + 1; }
            _ => {}
        }
    }
    out.push(&s[last..]);
    out
}

pub fn parse_decls(body: &str) -> Vec<Decl> {
    split_top(body, ';').into_iter().filter_map(|d| {
        let (p, v) = d.split_once(':')?;
        let (prop, mut val) = (if p.trim().starts_with("--") { p.trim().to_string() } else { p.trim().to_ascii_lowercase() }, v.trim());
        let imp = val.to_ascii_lowercase().ends_with("!important");
        if imp { val = val[..val.len() - 10].trim(); }
        (!prop.is_empty()).then(|| Decl { prop, val: val.to_string(), imp })
    }).collect()
}

fn parse_rules(css: &str, out: &mut Vec<Rule>, order: &mut usize, faces: &mut Vec<FontFaceRule>) {
    let (mut i, len) = (0, css.len());
    while i < len {
        let rest = &css[i..];
        let skipped = rest.len() - rest.trim_start().len();
        i += skipped;
        if i >= len { break; }
        let rest = &css[i..];
        if rest.starts_with('@') {
            let (semi, brace) = (rest.find(';'), rest.find('{'));
            match (semi, brace) {
                (Some(s), b) if b.is_none_or(|b| s < b) => i += s + 1,
                (_, Some(b)) => {
                    let end = matching_brace(css, i + b);
                    if rest.starts_with("@media") && media_ok(&rest[6..b]) { parse_rules(&css[i + b + 1..end.min(len)], out, order, faces); }
                    if rest[..b].trim() == "@font-face" { faces.extend(parse_face(&css[i + b + 1..end.min(len)])); }
                    i = end + 1;
                }
                _ => break,
            }
            continue;
        }
        let Some(b) = rest.find('{') else { break };
        let end = matching_brace(css, i + b);
        let (sel, body) = (&rest[..b], &css[i + b + 1..end.min(len)]);
        i = end + 1;
        let decls = Rc::new(parse_decls(body));
        if decls.is_empty() { continue; }
        for s in split_top(sel, ',') {
            if let Some(c) = parse_complex(s) {
                out.push(Rule { spec: specificity(&c), order: *order, sel: c, decls: decls.clone() });
                *order += 1;
            }
        }
    }
}

fn parse_face(body: &str) -> Option<FontFaceRule> {
    let d = parse_decls(body);
    let get = |p: &str| d.iter().rev().find(|x| x.prop == p).map(|x| x.val.as_str());
    let family = get("font-family")?.trim().trim_matches(['"', '\'']).trim().to_ascii_lowercase();
    let srcs: Vec<(String, String)> = split_top(get("src")?, ',').into_iter().filter_map(|part| {
        let p = part.find("url(")?;
        let rest = &part[p + 4..];
        let url = rest[..rest.find(')')?].trim().trim_matches(['"', '\'']).to_string();
        let fmt = part.find("format(").and_then(|f| part[f + 7..].find(')').map(|e| part[f + 7..f + 7 + e].trim().trim_matches(['"', '\'']).to_ascii_lowercase())).unwrap_or_default();
        Some((url, fmt))
    }).collect();
    let weight = get("font-weight").unwrap_or("400");
    let bold = matches!(weight, "bold" | "bolder") || weight.split_whitespace().next().and_then(|w| w.parse::<u32>().ok()).is_some_and(|w| w >= 600);
    (!family.is_empty() && !srcs.is_empty()).then(|| FontFaceRule { family, bold, italic: get("font-style").is_some_and(|v| v.starts_with("italic") || v.starts_with("oblique")), srcs })
}

impl Sheet {
    pub fn has_face(&self, family: &str) -> bool { !self.faces.is_empty() && self.faces.iter().any(|f| f.family == family) }

    /// Sheets are concatenated in cascade order (earlier = lower precedence on ties).
    pub fn new(sheets: &[String]) -> Sheet {
        let (mut rules, mut order) = (vec![], 0);
        let mut faces = vec![];
        for s in sheets { parse_rules(&strip_comments(s), &mut rules, &mut order, &mut faces); }
        let mut sh = Sheet { faces, ..Sheet::default() };
        for (k, r) in rules.iter().enumerate() {
            let key = &r.sel.parts.last().unwrap().1;
            if key.pseudo != 0 { sh.has_pseudo = true; }
            if r.sel.parts.iter().any(|p| p.1.state != 0) { sh.has_state = true; }
            if let Some(i) = &key.id { sh.by_id.entry(i.clone()).or_default().push(k); }
            else if let Some(c) = key.classes.first() { sh.by_class.entry(c.clone()).or_default().push(k); }
            else if let Some(t) = &key.tag { sh.by_tag.entry(t.clone()).or_default().push(k); }
            else { sh.other.push(k); }
        }
        sh.rules = rules;
        sh
    }

    /// Rules matching `id`; `which` 0 = the element itself, 1/2 = its ::before/::after.
    fn matching(&self, dom: &Dom, id: usize, which: u8) -> Vec<&Rule> {
        let mut out = Vec::new();
        let mut check = |ks: &[usize]| for &k in ks {
            let r = &self.rules[k];
            if r.sel.parts.last().is_some_and(|p| p.1.pseudo == which) && match_at(dom, id, &r.sel.parts, r.sel.parts.len() - 1) { out.push(r); }
        };
        check(&self.other);
        if let Some(v) = dom.attr(id, "id").and_then(|i| self.by_id.get(i)) { check(v); }
        for c in dom.attr(id, "class").unwrap_or("").split_whitespace() { if let Some(v) = self.by_class.get(c) { check(v); } }
        if let Some(v) = self.by_tag.get(&dom.nodes[id].tag) { check(v); }
        out
    }
}

#[derive(Clone)]
pub struct Style {
    pub none: bool,
    pub block: bool,
    pub list_item: bool,
    pub flex_row: bool,
    pub color: u32,
    pub size: f32,
    pub bold: bool,
    pub align: u8, // 0 left, 1 center, 2 right
    pub underline: bool,
    pub bullets: bool,
    pub collapse: bool, // border-collapse: collapse (inherited)
    /// Vertical margins (+ vertical padding) as flow gaps; horizontal margins/padding stay separate for box layout.
    pub mt: f32,
    pub mb: f32,
    pub ml: f32,
    pub mr: f32,
    pub pl: f32,
    pub pr: f32,
    pub auto_l: bool,
    pub auto_r: bool,
    pub width: Option<Len>,
    pub max_width: Option<Len>,
    pub pad: [f32; 4], // raw padding (top, right, bottom, left), used by table cells
    pub pad_set: bool,
    pub valign: u8, // 0 top, 1 middle, 2 bottom
    pub float: u8, // 0 none, 1 left, 2 right
    pub clear: u8, // bit 0 = left, bit 1 = right
    pub borders: [Option<(f32, u32)>; 4], // top, right, bottom, left: (width px, color)
    pub mono: bool, // monospace font (inherited)
    pub pre: bool,  // white-space: pre / pre-wrap (inherited)
    pub bg: Option<u32>,
    pub vars: Rc<HashMap<String, String>>, // custom properties (inherited)
    pub nowrap: bool, // white-space: nowrap (inherited)
    pub italic: bool, // font-style (inherited)
    pub fam: u16,     // web-font family id from font-family (inherited)
    pub tt: u8, // text-transform: 0 none, 1 upper, 2 lower, 3 capitalize (inherited)
    pub lh: f32, // line-height as a multiple of the font size (inherited)
    pub pt: f32, // vertical padding (already part of mt/mb as flow gaps); boxes with a background paint it
    pub pb: f32,
    pub bg_a: f32, // alpha of `bg` (rgba background)
    pub alpha: f32, // opacity of this element and its subtree, multiplied down (approximated by blending colors)
    pub ls: f32, // letter-spacing in px (inherited)
    pub bg_grad: Option<Arc<Grad>>,
    pub bg_img: Option<BgImage>,
    pub x: Ext,
}

/// `linear-gradient`: CSS angle in degrees (0 = up, 90 = right, 180 = down) and color stops at 0..1.
#[derive(Debug)]
pub struct Grad { pub angle: f32, pub stops: Vec<(f32, u32)>, pub radial: bool, pub circle: bool, pub at: (f32, f32) }

/// `background-image: url(...)` with its size, position (x, y) and repeat (x, y).
#[derive(Clone, Debug, PartialEq)]
pub struct BgImage { pub url: String, pub size: BgSize, pub pos: [Len; 2], pub repeat: (bool, bool) }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BgSize { Auto, Cover, Contain, Fixed(Option<Len>, Option<Len>) }

/// First `box-shadow` layer; `color` is u32::MAX until resolved to the element's color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow { pub dx: f32, pub dy: f32, pub blur: f32, pub spread: f32, pub color: u32, pub a: f32 }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Track { Px(f32), Pct(f32), Fr(f32) }

/// Non-inherited layout properties added after the first box model: positioning, inline-block and flex container/item settings.
#[derive(Clone, Default)]
pub struct Ext {
    pub pos: u8,                       // 0 static, 1 relative/sticky, 2 absolute
    pub off: [Option<Len>; 4],         // top, right, bottom, left
    pub inline_block: bool,
    pub wrap: bool, pub gap: f32,
    pub justify: u8,                   // flex: 0 start, 1 center, 2 end, 3 between, 4 around, 5 evenly
    pub align: u8,                     // align-items / align-self: 0 stretch(=start), 1 start, 2 center, 3 end
    pub grow: f32, pub shrink: f32, pub basis: Option<Len>,
    pub min_h: f32,                    // height / min-height in px (treated as a minimum)
    pub bb: bool,                      // box-sizing: border-box
    pub radius: f32,                   // border-radius in px (9999 for >= 50 %)
    pub indent: f32,                   // text-indent in px (first line of this block)
    pub z: i32,                        // z-index (positioned boxes only)
    pub fix_h: bool,                   // `height` (not just min-height) was given: with overflow clipping the box is exactly that tall
    pub clip: bool,                    // overflow other than visible
    pub scroll: bool,                  // overflow: auto | scroll
    pub shadow: Option<Shadow>,
    pub flex_col: bool, pub grid: bool,
    pub tracks: Vec<Track>, pub auto_min: Option<f32>, // grid-template-columns (auto_min: repeat(auto-fill, minmax(N, 1fr)))
    pub row_gap: Option<f32>, pub span: usize,         // grid: row-gap, grid-column span (0 = 1)
    pub va: f32,                       // vertical-align shift of inline text in em (super/sub), positive = up
    pub counter_reset: Vec<(String, i32)>, pub counter_inc: Vec<(String, i32)>,
    pub h_pct: Option<(f32, bool)>,    // percentage height/min-height (value, exact), resolved by the renderer against a definite parent height
    pub sticky: bool,                  // position: sticky (pos = 1)
    pub va_kind: u8,                   // vertical-align keyword for images: 1 middle, 2 top, 3 bottom
}

/// A CSS length that may depend on the containing block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Len { Px(f32), Pct(f32), /** percent + px, from calc() */ Mix(f32, f32) }

impl Style {
    pub fn root() -> Style {
        Style {
            none: false, block: true, list_item: false, flex_row: false, color: 0x111111, size: 16.0, bold: false, align: 0, underline: false, bullets: true,
            collapse: false, mt: 0.0, mb: 0.0, ml: 0.0, mr: 0.0, pl: 0.0, pr: 0.0, auto_l: false, auto_r: false, width: None, max_width: None,
            pad: [0.0; 4], pad_set: false, valign: 0, float: 0, clear: 0, borders: [None; 4], mono: false, pre: false, bg: None, lh: 1.35, vars: Rc::default(), nowrap: false, italic: false, fam: 0, tt: 0, pt: 0.0, pb: 0.0, bg_a: 1.0, alpha: 1.0, ls: 0.0, bg_grad: None, bg_img: None, x: Ext { shrink: 1.0, ..Ext::default() },
        }
    }
    fn inherit(&self) -> Style {
        Style {
            none: false, block: false, list_item: false, flex_row: false, mt: 0.0, mb: 0.0, ml: 0.0, mr: 0.0, pl: 0.0, pr: 0.0, auto_l: false, auto_r: false,
            width: None, max_width: None, pad: [0.0; 4], pad_set: false, valign: 0, float: 0, clear: 0, borders: [None; 4], bg: None, pt: 0.0, pb: 0.0, bg_a: 1.0, bg_grad: None, bg_img: None, x: Ext { shrink: 1.0, ..Ext::default() }, ..self.clone()
        }
    }
}

/// `1px solid #ccc` -> (width, color); None when the border is off. A missing color means "currentcolor" (u32::MAX).
fn border_value(v: &str, em: f32) -> Option<(f32, u32)> {
    let toks: Vec<&str> = v.split_whitespace().collect();
    if toks.iter().any(|t| matches!(*t, "none" | "hidden")) || toks.is_empty() { return None; }
    let mut w = 3.0;
    for t in &toks {
        match *t { "thin" => w = 1.0, "medium" => w = 3.0, "thick" => w = 5.0, _ => if let Some(px) = length(t, em) { w = px } }
    }
    let color = if let Some(p) = v.find("rgb") { parse_color(&v[p..=v[p..].find(')').map_or(v.len() - 1, |e| p + e)]) } else { toks.iter().find_map(|t| parse_color(t)) };
    (w > 0.0).then_some((w.min(20.0), color.unwrap_or(u32::MAX)))
}

#[derive(Clone, Copy)]
struct V { pct: f32, px: f32, num: bool }

impl V {
    fn scale(self, k: f32) -> V { V { pct: self.pct * k, px: self.px * k, num: self.num } }
}

/// Recursive-descent evaluator for calc()/min()/max()/clamp() bodies into a linear (percent, px) value.
struct Calc<'a> { c: &'a [u8], i: usize, em: f32 }

impl Calc<'_> {
    fn ws(&mut self) { while self.c.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) { self.i += 1; } }
    fn expr(&mut self) -> Option<V> {
        let mut a = self.term()?;
        loop {
            self.ws();
            let op = match self.c.get(self.i) { Some(&o @ (b'+' | b'-')) => o, _ => return Some(a) };
            self.i += 1;
            let b = self.term()?;
            if a.num != b.num { return None; }
            let k = if op == b'+' { 1.0 } else { -1.0 };
            a = V { pct: a.pct + k * b.pct, px: a.px + k * b.px, num: a.num };
        }
    }
    fn term(&mut self) -> Option<V> {
        let mut a = self.factor()?;
        loop {
            self.ws();
            let op = match self.c.get(self.i) { Some(&o @ (b'*' | b'/')) => o, _ => return Some(a) };
            self.i += 1;
            let b = self.factor()?;
            a = match (op, a.num, b.num) {
                (b'*', true, _) => b.scale(a.px),
                (b'*', _, true) => a.scale(b.px),
                (b'/', _, true) if b.px != 0.0 => a.scale(1.0 / b.px),
                _ => return None,
            };
        }
    }
    fn factor(&mut self) -> Option<V> {
        self.ws();
        let c = *self.c.get(self.i)?;
        match c {
            b'(' => { self.i += 1; let v = self.expr()?; self.ws(); if self.c.get(self.i) == Some(&b')') { self.i += 1; Some(v) } else { None } }
            b'-' => { self.i += 1; Some(self.factor()?.scale(-1.0)) }
            b'+' => { self.i += 1; self.factor() }
            c if c.is_ascii_digit() || c == b'.' => {
                let s0 = self.i;
                while self.c.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') { self.i += 1; }
                let n: f32 = std::str::from_utf8(&self.c[s0..self.i]).ok()?.parse().ok()?;
                let u0 = self.i;
                while self.c.get(self.i).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'%') { self.i += 1; }
                let em = self.em;
                Some(match std::str::from_utf8(&self.c[u0..self.i]).ok()? {
                    "" => V { pct: 0.0, px: n, num: true },
                    "px" => V { pct: 0.0, px: n, num: false },
                    "%" => V { pct: n, px: 0.0, num: false },
                    "em" => V { pct: 0.0, px: n * em, num: false },
                    "rem" => V { pct: 0.0, px: n * 16.0, num: false },
                    "pt" => V { pct: 0.0, px: n * 4.0 / 3.0, num: false },
                    "vw" | "vmax" => V { pct: 0.0, px: n * VIEWPORT_W / 100.0, num: false },
                    "vh" | "vmin" => V { pct: 0.0, px: n * 7.2, num: false },
                    "ch" | "ex" => V { pct: 0.0, px: n * em * 0.5, num: false },
                    _ => return None,
                })
            }
            c if c.is_ascii_alphabetic() => {
                let s0 = self.i;
                while self.c.get(self.i).is_some_and(|c| c.is_ascii_alphabetic()) { self.i += 1; }
                let name = std::str::from_utf8(&self.c[s0..self.i]).ok()?.to_ascii_lowercase();
                if self.c.get(self.i) != Some(&b'(') { return None; }
                let open = self.i;
                let (mut depth, mut end) = (0, None);
                for (k, &ch) in self.c.iter().enumerate().skip(open) {
                    if ch == b'(' { depth += 1 } else if ch == b')' { depth -= 1; if depth == 0 { end = Some(k); break; } }
                }
                let end = end?;
                let inner = std::str::from_utf8(&self.c[open + 1..end]).ok()?;
                self.i = end + 1;
                let args: Vec<V> = split_top(inner, ',').into_iter().map(|a| { let mut p = Calc { c: a.as_bytes(), i: 0, em: self.em }; let v = p.expr()?; p.ws(); (p.i == a.len()).then_some(v) }).collect::<Option<_>>()?;
                let plain = args.iter().all(|a| a.pct == 0.0);
                match (name.as_str(), args.len()) {
                    ("calc" | "", 1) => Some(args[0]),
                    ("min", n) if n >= 1 && plain => args.iter().copied().reduce(|a, b| if b.px < a.px { b } else { a }),
                    ("max", n) if n >= 1 && plain => args.iter().copied().reduce(|a, b| if b.px > a.px { b } else { a }),
                    ("clamp", 3) if plain => Some(V { px: args[1].px.max(args[0].px).min(args[2].px), ..args[1] }),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// (percent, px) of `calc(...)`/`min(...)`/`max(...)`/`clamp(...)`; None for anything else or on a syntax error.
fn calc_eval(v: &str, em: f32) -> Option<(f32, f32)> {
    let v = v.trim();
    if !["calc(", "min(", "max(", "clamp("].iter().any(|p| v.to_ascii_lowercase().starts_with(p)) { return None; }
    let mut p = Calc { c: v.as_bytes(), i: 0, em };
    let r = p.expr()?;
    p.ws();
    (p.i == v.len()).then_some((r.pct, r.px))
}

/// Replaces every `var(--name, fallback)` using `vars`; None when a variable is undefined and has no fallback.
fn subst_vars(v: &str, vars: &HashMap<String, String>, depth: u8) -> Option<String> {
    let (mut out, mut rest) = (String::new(), v);
    while let Some(p) = rest.find("var(") {
        out.push_str(&rest[..p]);
        let (mut d, mut end) = (0, None);
        for (k, c) in rest[p + 3..].char_indices() {
            if c == '(' { d += 1 } else if c == ')' { d -= 1; if d == 0 { end = Some(p + 3 + k); break; } }
        }
        let end = end?;
        let inner = &rest[p + 4..end];
        let (name, fallback) = match inner.split_once(',') { Some((n, f)) => (n.trim(), Some(f.trim())), None => (inner.trim(), None) };
        let val = vars.get(name).cloned().or_else(|| fallback.map(String::from))?;
        out.push_str(&if depth < 8 && val.contains("var(") { subst_vars(&val, vars, depth + 1)? } else { val });
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn len_value(v: &str, em: f32) -> Option<Len> {
    let v = v.trim();
    if let Some((pct, px)) = calc_eval(v, em) { return Some(if pct == 0.0 { Len::Px(px) } else if px == 0.0 { Len::Pct(pct) } else { Len::Mix(pct, px) }); }
    if let Some(p) = v.strip_suffix('%') { return p.trim().parse().ok().map(Len::Pct); }
    length(v, em).map(Len::Px)
}

/// Token index for (top, right, bottom, left) given 1-4 shorthand values.
fn expand4(n: usize) -> Option<[usize; 4]> {
    Some(match n { 1 => [0, 0, 0, 0], 2 => [0, 1, 0, 1], 3 => [0, 1, 2, 1], 4 => [0, 1, 2, 3], _ => return None })
}

fn length(v: &str, em: f32) -> Option<f32> {
    let v = v.trim();
    if let Some((pct, px)) = calc_eval(v, em) { return (pct == 0.0).then_some(px); }
    let end = v.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(v.len());
    let n: f32 = v[..end].parse().ok()?;
    Some(match &v[end..] { "px" | "" if end > 0 => n, "em" => n * em, "rem" => n * 16.0, "%" => n / 100.0 * em, "pt" => n * 4.0 / 3.0, "vw" | "vmax" => n * VIEWPORT_W / 100.0, "vh" | "vmin" => n * 7.2, "ch" | "ex" => n * em * 0.5, _ => return None })
}

/// Box shorthand -> (values, auto flags) as (top, right, bottom, left).
fn sides(v: &str, em: f32) -> Option<([f32; 4], [bool; 4])> {
    let t: Vec<&str> = split_ws(v);
    let ix = expand4(t.len())?;
    let (mut vals, mut auto) = ([0f32; 4], [false; 4]);
    for k in 0..4 {
        auto[k] = t[ix[k]] == "auto";
        vals[k] = if auto[k] { 0.0 } else { length(t[ix[k]], em)? };
    }
    Some((vals, auto))
}

pub fn parse_color(v: &str) -> Option<u32> { parse_color_a(v).map(|c| c.0) }

/// Color and alpha (0..1); fully transparent colors are None.
pub fn parse_color_a(v: &str) -> Option<(u32, f32)> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(h) = v.strip_prefix('#') {
        let a = if h.len() == 8 { u8::from_str_radix(&h[6..], 16).ok()? as f32 / 255.0 } else if h.len() == 4 { h[3..].chars().next()?.to_digit(16)? as f32 / 15.0 } else { 1.0 };
        if a == 0.0 { return None; }
        return match h.len() {
            3 | 4 => { let d: Vec<u32> = h.chars().take(3).map(|c| c.to_digit(16)).collect::<Option<_>>()?; Some((d[0] * 17 << 16 | d[1] * 17 << 8 | d[2] * 17, a)) }
            6 | 8 => u32::from_str_radix(&h[..6], 16).ok().map(|c| (c, a)),
            _ => None,
        };
    }
    if v.starts_with("rgb") {
        let inner = &v[v.find('(')? + 1..v.rfind(')')?];
        let toks: Vec<&str> = inner.split([',', ' ', '/']).filter(|x| !x.is_empty()).collect();
        if toks.len() < 3 { return None; }
        let ch = |x: &str| x.strip_suffix('%').map_or_else(|| x.parse::<f32>().ok(), |p| p.parse::<f32>().ok().map(|n| n * 2.55));
        let (r, g, b) = (ch(toks[0])?, ch(toks[1])?, ch(toks[2])?);
        let a = match toks.get(3) { Some(x) => x.strip_suffix('%').map_or_else(|| x.parse::<f32>().ok(), |p| p.parse::<f32>().ok().map(|n| n / 100.0))?, None => 1.0 }.clamp(0.0, 1.0);
        if a == 0.0 { return None; }
        return Some(((r.clamp(0.0, 255.0) as u32) << 16 | (g.clamp(0.0, 255.0) as u32) << 8 | b.clamp(0.0, 255.0) as u32, a));
    }
    Some((match v.as_str() {
        "black" => 0, "white" => 0xFFFFFF, "red" => 0xFF0000, "green" => 0x008000, "blue" => 0x0000FF, "gray" | "grey" => 0x808080,
        "silver" => 0xC0C0C0, "yellow" => 0xFFFF00, "orange" => 0xFFA500, "purple" => 0x800080, "navy" => 0x000080, "teal" => 0x008080,
        "maroon" => 0x800000, "lime" => 0x00FF00, "aqua" | "cyan" => 0x00FFFF, "fuchsia" | "magenta" => 0xFF00FF, "olive" => 0x808000,
        "darkgray" | "darkgrey" => 0xA9A9A9, "lightgray" | "lightgrey" => 0xD3D3D3, "brown" => 0xA52A2A, "pink" => 0xFFC0CB,
        _ => return None,
    }, 1.0))
}

/// First color of a `background` value and its alpha; gradients give their first stop (a flat fallback under the real gradient).
fn background_color(v: &str) -> Option<(u32, f32)> {
    if let Some(p) = v.find("gradient(") {
        let inner = &v[p + 9..];
        if let Some(r) = inner.find("rgb") { return parse_color_a(&inner[r..=inner[r..].find(')').map_or(inner.len() - 1, |e| r + e)]).map(|c| (c.0, 1.0)); }
        return inner.split([',', '(', ')', ' ']).find_map(parse_color).map(|c| (c, 1.0));
    }
    if let Some(r) = v.find("rgb") { return parse_color_a(&v[r..=v[r..].find(')').map_or(v.len() - 1, |e| r + e)]); }
    split_ws(v).into_iter().find_map(parse_color_a)
}

/// `linear-gradient(angle|to side, stop, stop...)`; None for anything else (radial and conic fall back to the first stop).
fn parse_gradient(v: &str) -> Option<Grad> {
    let (radial, p) = match v.find("radial-gradient(") { Some(p) => (true, p), None => (false, v.find("linear-gradient(")?) };
    let body = &v[p + 16..];
    let (mut depth, mut end) = (1, None);
    for (i, c) in body.char_indices() { if c == '(' { depth += 1 } else if c == ')' { depth -= 1; if depth == 0 { end = Some(i); break; } } }
    let args = split_top(&body[..end?], ',');
    let first = args.first()?.trim();
    let (mut angle, mut from) = (180.0f32, 0);
    let (mut circle, mut at) = (false, (0.5f32, 0.5f32));
    if radial {
        if !split_ws(first).iter().any(|t| parse_color(t).is_some()) {
            circle = first.contains("circle");
            if let Some(i) = first.find("at ") {
                let pct = |l: Len| if let Len::Pct(p) = l { p / 100.0 } else { 0.5 };
                let [x, y] = parse_bg_pos(&split_ws(&first[i + 3..]), 16.0);
                at = (pct(x), pct(y));
            }
            from = 1;
        }
    } else if let Some(dir) = first.strip_prefix("to ") {
        let (t, r, b, l) = (dir.contains("top"), dir.contains("right"), dir.contains("bottom"), dir.contains("left"));
        angle = match (t, r, b, l) { (true, false, false, false) => 0.0, (false, true, false, false) => 90.0, (false, false, true, false) => 180.0, (false, false, false, true) => 270.0, (true, true, _, _) => 45.0, (_, true, true, _) => 135.0, (_, _, true, true) => 225.0, _ => 315.0 };
        from = 1;
    } else if let Some((n, u)) = ["deg", "turn", "rad", "grad"].iter().find_map(|u| first.strip_suffix(u).map(|n| (n, *u))) {
        let x: f32 = n.trim().parse().ok()?;
        angle = match u { "turn" => x * 360.0, "rad" => x.to_degrees(), "grad" => x * 0.9, _ => x };
        from = 1;
    }
    let mut stops: Vec<(Option<f32>, u32)> = vec![];
    for a in &args[from..] {
        let toks = split_ws(a.trim());
        let Some(c) = toks.iter().find_map(|t| parse_color(t)) else { continue };
        let pos = toks.iter().find_map(|t| t.strip_suffix('%').and_then(|n| n.parse::<f32>().ok())).map(|p| p / 100.0);
        stops.push((pos, c));
    }
    if stops.len() < 2 { return None; }
    let n = stops.len();
    let mut out: Vec<(f32, u32)> = stops.iter().enumerate().map(|(i, (pos, c))| (pos.unwrap_or(i as f32 / (n - 1) as f32), *c)).collect();
    for i in 1..n { if out[i].0 < out[i - 1].0 { out[i].0 = out[i - 1].0; } }
    Some(Grad { angle: angle.rem_euclid(360.0), stops: out, radial, circle, at })
}

fn css_url(v: &str) -> Option<String> {
    let rest = &v[v.find("url(")? + 4..];
    Some(rest[..rest.find(')')?].trim().trim_matches(['"', '\'']).to_string())
}

/// `background-position` tokens (keywords, lengths, percentages) as (x, y); a single value centers the other axis.
fn parse_bg_pos(toks: &[&str], em: f32) -> [Len; 2] {
    let kw = |t: &str| match t { "left" | "top" => Some(0.0), "center" => Some(50.0), "right" | "bottom" => Some(100.0), _ => None };
    let one = |t: &str| kw(t).map(Len::Pct).or_else(|| len_value(t, em));
    let (mut x, mut y) = (Len::Pct(0.0), Len::Pct(0.0));
    match toks {
        [a] => match *a {
            "top" | "bottom" => { x = Len::Pct(50.0); y = one(a).unwrap_or(y); }
            "center" => { x = Len::Pct(50.0); y = Len::Pct(50.0); }
            _ => { x = one(a).unwrap_or(x); y = Len::Pct(50.0); }
        },
        [a, b, ..] => {
            let (hx, hy) = if matches!(*a, "top" | "bottom") || matches!(*b, "left" | "right") { (*b, *a) } else { (*a, *b) };
            x = one(hx).unwrap_or(x);
            y = one(hy).unwrap_or(y);
        }
        _ => {}
    }
    [x, y]
}

fn parse_bg_size(toks: &[&str], em: f32) -> BgSize {
    match toks {
        ["cover"] => BgSize::Cover,
        ["contain"] => BgSize::Contain,
        [a] => BgSize::Fixed(len_value(a, em), None),
        [a, b, ..] => BgSize::Fixed(len_value(a, em), len_value(b, em)),
        _ => BgSize::Auto,
    }
}

fn parse_bg_repeat(toks: &[&str]) -> (bool, bool) {
    match toks {
        ["no-repeat"] => (false, false),
        ["repeat-x"] => (true, false),
        ["repeat-y"] => (false, true),
        [a, b] => (*a != "no-repeat", *b != "no-repeat"),
        _ => (true, true),
    }
}

/// Image layer state while a rule's background properties are applied in order.
#[derive(Clone)]
struct BgParts { url: Option<String>, size: BgSize, pos: [Len; 2], repeat: (bool, bool) }

impl BgParts {
    fn new() -> BgParts { BgParts { url: None, size: BgSize::Auto, pos: [Len::Pct(0.0); 2], repeat: (true, true) } }

    /// The first layer of a `background` shorthand: `url(..) 50% 0 / cover no-repeat #fff`.
    fn shorthand(v: &str, em: f32) -> BgParts {
        let mut b = BgParts::new();
        let (mut layer, mut depth) = (String::new(), 0);
        for c in split_top(v, ',').into_iter().next().unwrap_or("").chars() { // a `/` outside parentheses separates position from size
            match c { '(' => { depth += 1; layer.push(c); } ')' => { depth -= 1; layer.push(c); } '/' if depth == 0 => layer.push_str(" / "), _ => layer.push(c) }
        }
        b.url = css_url(&layer);
        let (mut pos, mut size, mut rep, mut after_slash) = (vec![], vec![], vec![], false);
        for t in split_ws(&layer) {
            if t == "/" { after_slash = true; continue; }
            if t.starts_with("url(") || t.contains("gradient(") { continue; }
            if matches!(t, "no-repeat" | "repeat" | "repeat-x" | "repeat-y" | "space" | "round") { rep.push(t); }
            else if after_slash && size.len() < 2 && (matches!(t, "cover" | "contain" | "auto") || len_value(t, em).is_some()) { size.push(t); }
            else if matches!(t, "left" | "right" | "top" | "bottom" | "center") || (parse_color(t).is_none() && len_value(t, em).is_some()) { if !after_slash { pos.push(t); } }
        }
        if !pos.is_empty() { b.pos = parse_bg_pos(&pos, em); }
        if !size.is_empty() { b.size = parse_bg_size(&size, em); }
        if !rep.is_empty() { b.repeat = parse_bg_repeat(&rep); }
        b
    }
}

/// `0 2px 4px 1px rgba(0,0,0,.2)` (first of a comma list); inset shadows are skipped.
fn parse_shadow(v: &str, em: f32) -> Option<Shadow> {
    let first = split_top(v, ',').into_iter().next()?.trim().to_string();
    if first.contains("inset") || first == "none" { return None; }
    let toks = split_ws(&first);
    let nums: Vec<f32> = toks.iter().filter_map(|t| length(t, em)).collect();
    if nums.len() < 2 { return None; }
    let (color, a) = toks.iter().find_map(|t| parse_color_a(t)).unwrap_or((u32::MAX, 0.35));
    Some(Shadow { dx: nums[0], dy: nums[1], blur: nums.get(2).copied().unwrap_or(0.0).clamp(0.0, 60.0), spread: nums.get(3).copied().unwrap_or(0.0).clamp(-30.0, 60.0), color, a })
}

/// `grid-template-columns`: tracks plus the minimum width of a `repeat(auto-fill, minmax(N, 1fr))` pattern.
fn parse_tracks(v: &str, em: f32) -> (Vec<Track>, Option<f32>) {
    let (mut out, mut auto) = (vec![], None);
    for t in split_ws(v) {
        if let Some(inner) = t.strip_prefix("repeat(").and_then(|r| r.strip_suffix(')')) {
            let (n, body) = inner.split_once(',').unwrap_or((inner, ""));
            let (n, body) = (n.trim(), body.trim());
            let (sub, _) = parse_tracks(body, em);
            if n == "auto-fill" || n == "auto-fit" {
                let min = body.strip_prefix("minmax(").and_then(|m| m.split(',').next()).and_then(|m| length(m, em)).or_else(|| length(body, em));
                auto = min.filter(|m| *m > 0.0);
                if auto.is_none() { out.extend(sub); }
            } else if let Ok(k) = n.parse::<usize>() { for _ in 0..k.min(24) { out.extend(sub.iter().copied()); } }
        } else if let Some(n) = t.strip_suffix("fr") { out.push(Track::Fr(n.parse().unwrap_or(1.0))); }
        else if let Some(m) = t.strip_prefix("minmax(").and_then(|m| m.strip_suffix(')')) {
            let hi = m.split(',').nth(1).unwrap_or("").trim();
            out.push(if let Some(n) = hi.strip_suffix("fr") { Track::Fr(n.parse().unwrap_or(1.0)) } else { length(hi, em).map_or(Track::Fr(1.0), Track::Px) });
        }
        else if let Some(p) = t.strip_suffix('%') { out.push(Track::Pct(p.parse().unwrap_or(0.0))); }
        else if let Some(px) = length(t, em) { out.push(Track::Px(px)); }
        else if matches!(t, "auto" | "min-content" | "max-content") || t.starts_with("fit-content") { out.push(Track::Fr(1.0)); }
    }
    (out, auto)
}

fn parse_counters(v: &str, default: i32) -> Vec<(String, i32)> {
    let mut out: Vec<(String, i32)> = vec![];
    for t in split_ws(v) {
        if let Ok(n) = t.parse::<i32>() { if let Some(l) = out.last_mut() { l.1 = n; } }
        else if t != "none" { out.push((t.to_string(), default)); }
    }
    out
}

fn font_size(v: &str, parent: f32) -> Option<f32> {
    Some(match v.trim() {
        "xx-small" => 9.0, "x-small" => 10.0, "small" => 13.0, "medium" => 16.0, "large" => 18.0, "x-large" => 24.0, "xx-large" => 32.0,
        "smaller" => parent * 0.83, "larger" => parent * 1.2,
        x => length(x, parent)?,
    })
}

pub fn compute(dom: &Dom, sheet: &Sheet, id: usize, parent: &Style) -> Style { cascade(dom, id, parent, sheet.matching(dom, id, 0), true, sheet) }

/// Style and text of the `::before` (1) / `::after` (2) pseudo-element of `id`; None without text `content`.
pub fn pseudo(dom: &Dom, sheet: &Sheet, id: usize, which: u8, parent: &Style) -> Option<(Style, String)> {
    if !sheet.has_pseudo { return None; }
    let mut rules = sheet.matching(dom, id, which);
    rules.sort_by_key(|r| (r.spec, r.order));
    let text = rules.iter().flat_map(|r| r.decls.iter()).filter(|d| d.prop == "content").next_back().and_then(|d| content_text(&d.val, dom, id))?;
    (!text.is_empty()).then(|| (cascade(dom, id, parent, rules, false, sheet), text))
}

/// `content` value: concatenated strings and `attr(x)` resolved; `counter(name)` / `counters(name, sep)` become
/// `\u{1}name\u{2}sep\u{3}` markers that the renderer (which owns the counters) replaces.
fn content_text(v: &str, dom: &Dom, id: usize) -> Option<String> {
    let mut out = String::new();
    let mut any = false;
    for tok in content_tokens(v) {
        let t = tok.trim();
        if let Some(inner) = t.strip_prefix("attr(").and_then(|r| r.strip_suffix(')')) { out.push_str(dom.attr(id, inner.trim()).unwrap_or("")); any = true; }
        else if let Some(inner) = t.strip_prefix("counter(").and_then(|r| r.strip_suffix(')')) { out.push_str(&format!("\u{1}{}\u{2}\u{3}", inner.split(',').next().unwrap_or("").trim())); any = true; }
        else if let Some(inner) = t.strip_prefix("counters(").and_then(|r| r.strip_suffix(')')) {
            let mut it = inner.splitn(2, ',');
            let name = it.next().unwrap_or("").trim();
            let sep = it.next().map_or(String::new(), |s| content_string(s.trim()).unwrap_or_default());
            out.push_str(&format!("\u{1}{name}\u{2}{sep}\u{3}")); any = true;
        }
        else if let Some(sv) = content_string(t) { out.push_str(&sv); any = true; }
    }
    // a string with spaces is split by split_ws above only outside quotes; whole quoted strings keep their spaces
    any.then_some(out)
}

/// Whitespace split that keeps quoted strings and `func(...)` calls whole.
fn content_tokens(v: &str) -> Vec<&str> {
    let (mut out, mut depth, mut quote, mut start) = (vec![], 0i32, None::<char>, None::<usize>);
    for (i, c) in v.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => { quote = Some(c); start.get_or_insert(i); }
            (None, '(') => { depth += 1; start.get_or_insert(i); }
            (None, ')') => depth -= 1,
            (None, c) if c.is_whitespace() && depth <= 0 => { if let Some(st) = start.take() { out.push(&v[st..i]); } }
            _ => { start.get_or_insert(i); }
        }
    }
    if let Some(st) = start { out.push(&v[st..]); }
    out
}

fn content_string(v: &str) -> Option<String> {
    let v = v.trim();
    let q = v.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let inner = &v[1..];
    let end = inner.rfind(q)?;
    let (mut out, mut it) = (String::new(), inner[..end].chars().peekable());
    while let Some(c) = it.next() {
        if c != '\\' { out.push(c); continue; }
        let mut hex = String::new();
        while hex.len() < 6 && it.peek().is_some_and(|d| d.is_ascii_hexdigit()) { hex.extend(it.next()); }
        if hex.is_empty() { out.extend(it.next()); } else {
            if it.peek() == Some(&' ') { it.next(); }
            out.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
        }
    }
    Some(out)
}

fn cascade(dom: &Dom, id: usize, parent: &Style, rules: Vec<&Rule>, element: bool, sheet: &Sheet) -> Style {
    let mut s = parent.inherit();
    // Presentational attributes (lowest precedence, CSS overrides below).
    if element {
        if let Some(c) = dom.attr(id, "bgcolor").and_then(parse_color) { s.bg = Some(c); }
        if dom.nodes[id].tag == "font" { if let Some(c) = dom.attr(id, "color").and_then(parse_color) { s.color = c; } }
        if matches!(dom.nodes[id].tag.as_str(), "td" | "th" | "tr" | "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "caption") {
            match dom.attr(id, "align").map(str::to_ascii_lowercase).as_deref() { Some("center") => s.align = 1, Some("right") => s.align = 2, Some("left") => s.align = 0, _ => {} }
        }
    }
    let mut matched: Vec<(bool, u32, usize, &Decl)> = rules.into_iter().flat_map(|r| r.decls.iter().map(move |d| (d.imp, r.spec, r.order, d))).collect();
    let inline = if element { dom.attr(id, "style").map(parse_decls).unwrap_or_default() } else { vec![] };
    matched.extend(inline.iter().map(|d| (d.imp, u32::MAX, usize::MAX, d)));
    matched.sort_by_key(|&(imp, spec, ord, _)| (imp, spec, ord));
    // custom properties first (they inherit), then every value with var() substituted
    for (_, _, _, d) in matched.iter().filter(|m| m.3.prop.starts_with("--")) {
        if let Some(v) = if d.val.contains("var(") { subst_vars(&d.val, &s.vars, 0) } else { Some(d.val.clone()) } { Rc::make_mut(&mut s.vars).insert(d.prop.clone(), v); }
    }
    let vals: Vec<Option<Cow<str>>> = matched.iter().map(|m| if m.3.val.contains("var(") { subst_vars(&m.3.val, &s.vars, 0).map(Cow::Owned) } else { Some(Cow::Borrowed(m.3.val.as_str())) }).collect();
    for (k, (_, _, _, d)) in matched.iter().enumerate() {
        let Some(val) = vals[k].as_deref() else { continue };
        if d.prop == "font-size" {
            if let Some(px) = font_size(val, parent.size) { s.size = px.clamp(0.0, 100.0); if px < 1.0 { s.none = true; } }
        }
    }
    let (mut disp, mut col) = (None::<&str>, false);
    let (mut m, mut p, mut auto, mut pad_set, mut overflow_hidden) = ([0f32; 4], [0f32; 4], [false; 4], false, false);
    let mut opacity = None::<f32>;
    let mut bgp = BgParts::new();
    let em = s.size;
    for (k, (_, _, _, d)) in matched.iter().enumerate() {
        let Some(v) = vals[k].as_deref() else { continue };
        match d.prop.as_str() {
            "height" | "min-height" => match len_value(v, em) {
                Some(Len::Px(p)) => { s.x.min_h = s.x.min_h.max(p.clamp(0.0, 20000.0)); if d.prop == "height" { s.x.fix_h = true; } }
                Some(Len::Pct(p)) => s.x.h_pct = Some((p, d.prop == "height")),
                _ => {}
            },
            "border-radius" => s.x.radius = match v.split_whitespace().next() { Some(t) if t.ends_with('%') => if t.trim_end_matches('%').parse::<f32>().is_ok_and(|p| p >= 50.0) { 9999.0 } else { 0.0 }, Some(t) => length(t, em).unwrap_or(0.0).clamp(0.0, 9999.0), None => 0.0 },
            "box-sizing" => s.x.bb = v == "border-box",
            "text-transform" => s.tt = match v { "uppercase" => 1, "lowercase" => 2, "capitalize" => 3, _ => 0 },
            "display" => disp = Some(v),
            "flex-direction" => col = v.starts_with("column"),
            "color" => if let Some(c) = parse_color(v) { s.color = c },
            "font-weight" => s.bold = matches!(v, "bold" | "bolder") || v.parse::<u32>().is_ok_and(|n| n >= 600),
            "text-align" => s.align = match v { "center" => 1, "right" | "end" => 2, _ => 0 },
            "text-decoration" | "text-decoration-line" => s.underline = v.contains("underline"),
            "list-style" | "list-style-type" => s.bullets = !v.starts_with("none"),
            "visibility" => if v == "hidden" || v == "collapse" { s.none = true },
            "opacity" => opacity = v.strip_suffix('%').map_or_else(|| v.parse::<f32>().ok(), |p| p.parse::<f32>().ok().map(|n| n / 100.0)).map(|o| o.clamp(0.0, 1.0)),
            "background" | "background-color" => {
                if let Some((c, a)) = background_color(v) { (s.bg, s.bg_a) = (Some(c), a); }
                if d.prop == "background" || v.contains("gradient(") { s.bg_grad = parse_gradient(v).map(Arc::new); }
                if d.prop == "background" { bgp = BgParts::shorthand(v, em); }
            }
            "background-image" => if v.contains("gradient(") {
                if s.bg.is_none() { if let Some((c, _)) = background_color(v) { (s.bg, s.bg_a) = (Some(c), 1.0); } }
                s.bg_grad = parse_gradient(v).map(Arc::new);
                bgp.url = None;
            } else { bgp.url = css_url(v); if bgp.url.is_some() { s.bg_grad = None; } },
            "background-size" => bgp.size = parse_bg_size(&split_ws(v), em),
            "background-position" => bgp.pos = parse_bg_pos(&split_ws(v), em),
            "background-repeat" => bgp.repeat = parse_bg_repeat(&split_ws(v)),
            "margin" => if let Some((vals, a)) = sides(v, em) { (m, auto) = (vals, a) },
            "margin-top" => { m[0] = length(v, em).unwrap_or(0.0); auto[0] = v == "auto"; }
            "margin-right" => { m[1] = length(v, em).unwrap_or(0.0); auto[1] = v == "auto"; }
            "margin-bottom" => { m[2] = length(v, em).unwrap_or(0.0); auto[2] = v == "auto"; }
            "margin-left" => { m[3] = length(v, em).unwrap_or(0.0); auto[3] = v == "auto"; }
            "padding" => if let Some((vals, _)) = sides(v, em) { p = vals; pad_set = true; },
            "padding-top" => { p[0] = length(v, em).unwrap_or(0.0); pad_set = true; }
            "padding-right" => { p[1] = length(v, em).unwrap_or(0.0); pad_set = true; }
            "padding-bottom" => { p[2] = length(v, em).unwrap_or(0.0); pad_set = true; }
            "padding-left" => { p[3] = length(v, em).unwrap_or(0.0); pad_set = true; }
            "width" => s.width = len_value(v, em),
            "max-width" => s.max_width = len_value(v, em),
            "vertical-align" => {
                s.valign = match v { "middle" => 1, "bottom" => 2, _ => 0 };
                s.x.va_kind = match v { "middle" => 1, "top" | "text-top" => 2, "bottom" | "text-bottom" => 3, _ => 0 };
                s.x.va = match v { "super" => 0.33, "sub" => -0.2, "text-top" | "top" => 0.15, "text-bottom" | "bottom" => -0.15, x => length(x, em).map_or(0.0, |p| p / em.max(1.0)).clamp(-2.0, 2.0) };
            }
            "z-index" => s.x.z = v.parse::<i32>().unwrap_or(0).clamp(-1000, 100000),
            "box-shadow" => s.x.shadow = parse_shadow(v, em),
            "letter-spacing" => s.ls = if v == "normal" { 0.0 } else { length(v, em).unwrap_or(0.0).clamp(-5.0, 50.0) },
            "grid-template-columns" => (s.x.tracks, s.x.auto_min) = parse_tracks(v, em),
            "grid-column" | "grid-column-start" | "grid-column-end" => if let Some(n) = v.split("span").nth(1).and_then(|n| n.split_whitespace().next()).and_then(|n| n.parse::<usize>().ok()) { s.x.span = n.clamp(1, 24); },
            "row-gap" | "grid-row-gap" => s.x.row_gap = length(v, em).map(|g| g.clamp(0.0, 200.0)),
            "counter-reset" | "counter-set" => s.x.counter_reset = parse_counters(v, 0),
            "counter-increment" => s.x.counter_inc = parse_counters(v, 1),
            "border-collapse" => s.collapse = v == "collapse",
            // shortcut: sticky acts as relative (right while the page is not scrolled); fixed boxes stay in the viewport at paint time
            "position" => { s.x.sticky = v == "sticky"; match v { "fixed" => s.x.pos = 3, "absolute" => s.x.pos = 2, "relative" | "sticky" => s.x.pos = 1, _ => s.x.pos = 0 } }
            "top" => s.x.off[0] = len_value(v, em),
            "right" => s.x.off[1] = len_value(v, em),
            "bottom" => s.x.off[2] = len_value(v, em),
            "left" => s.x.off[3] = len_value(v, em),
            "inset" => if let Some((vals, _)) = sides(v, em) { s.x.off = vals.map(|p| Some(Len::Px(p))) },
            "line-height" => s.lh = match v { "normal" => 1.35, x => x.parse::<f32>().ok().or_else(|| len_value(x, em).map(|l| match l { Len::Px(p) => p / em.max(1.0), Len::Pct(p) => p / 100.0, Len::Mix(..) => 1.35 })).unwrap_or(1.35).clamp(0.8, 4.0) },
            "flex-wrap" => s.x.wrap = v.starts_with("wrap"),
            "flex-flow" => { col = v.starts_with("column"); s.x.wrap = v.contains("wrap") && !v.contains("nowrap"); }
            "gap" | "column-gap" | "grid-gap" => {
                let t = split_ws(v);
                s.x.gap = length(t.last().copied().unwrap_or(""), em).unwrap_or(0.0).clamp(0.0, 200.0);
                if d.prop != "column-gap" { s.x.row_gap = t.first().and_then(|r| length(r, em)).map(|g| g.clamp(0.0, 200.0)); }
            }
            "justify-content" => s.x.justify = match v { "center" => 1, "flex-end" | "end" | "right" => 2, "space-between" => 3, "space-around" => 4, "space-evenly" => 5, _ => 0 },
            "align-items" | "align-self" => s.x.align = match v { "flex-start" | "start" | "baseline" => 1, "center" => 2, "flex-end" | "end" => 3, _ => 0 },
            "flex-grow" => s.x.grow = v.parse().unwrap_or(0.0),
            "flex-shrink" => s.x.shrink = v.parse().unwrap_or(1.0),
            "flex-basis" => s.x.basis = if v == "auto" { None } else { len_value(v, em) },
            "flex" => {
                let t: Vec<&str> = v.split_whitespace().collect();
                (s.x.grow, s.x.shrink, s.x.basis) = match t.as_slice() {
                    ["none"] => (0.0, 0.0, None),
                    ["auto"] => (1.0, 1.0, None),
                    [g] if g.parse::<f32>().is_ok() => (g.parse().unwrap_or(0.0), 1.0, Some(Len::Px(0.0))),
                    [g, sh] if sh.parse::<f32>().is_ok() => (g.parse().unwrap_or(0.0), sh.parse().unwrap_or(1.0), Some(Len::Px(0.0))),
                    [g, sh, b] => (g.parse().unwrap_or(0.0), sh.parse().unwrap_or(1.0), len_value(b, em)),
                    [g, b] => (g.parse().unwrap_or(0.0), 1.0, len_value(b, em)),
                    _ => (0.0, 1.0, None),
                };
            }
            // "visually hidden" utility hack (skip links, sr-only): clipped away or a 1px box with overflow hidden
            "clip" => if v.replace(' ', "").starts_with("rect(0") { s.none = true },
            "overflow" | "overflow-y" => {
                if d.prop == "overflow" { overflow_hidden = v == "hidden"; }
                let y = v.split_whitespace().last().unwrap_or(v); // `overflow: hidden auto` scrolls vertically
                s.x.clip = y != "visible";
                s.x.scroll = matches!(y, "auto" | "scroll");
            }
            "float" => s.float = match v { "left" => 1, "right" => 2, _ => 0 },
            "clear" => s.clear = match v { "left" => 1, "right" => 2, "both" => 3, _ => 0 },
            "border" => s.borders = [border_value(v, em); 4],
            "border-top" => s.borders[0] = border_value(v, em),
            "border-right" => s.borders[1] = border_value(v, em),
            "border-bottom" => s.borders[2] = border_value(v, em),
            "border-left" => s.borders[3] = border_value(v, em),
            "font-family" => {
                let l = v.to_ascii_lowercase();
                s.mono = ["monospace", "consolas", "courier", "menlo", "mono"].iter().any(|k| l.contains(k));
                // the first listed family that the page defines with @font-face
                s.fam = split_top(&l, ',').into_iter().map(|n| n.trim().trim_matches(['"', '\'']).trim()).find(|n| sheet.has_face(n)).map_or(0, crate::gfx::family_id);
            }
            "font-style" => s.italic = v.starts_with("italic") || v.starts_with("oblique"),
            "white-space" => { s.pre = matches!(v, "pre" | "pre-wrap" | "break-spaces"); s.nowrap = v == "nowrap"; }
            // large negative indents are the image-replacement trick for hiding text
            "text-indent" => if let Some(Len::Px(p)) = len_value(v, em) { if p <= -500.0 { s.none = true } else { s.x.indent = p.clamp(-200.0, 400.0) } },
            _ => {}
        }
    }
    match disp {
        Some("none") => s.none = true,
        Some(d) => {
            s.block = !matches!(d, "inline" | "inline-block" | "inline-flex" | "inline-grid" | "inline-table" | "contents" | "table-cell");
            s.list_item = d == "list-item";
            s.flex_row = matches!(d, "flex" | "inline-flex") && !col;
            s.x.flex_col = matches!(d, "flex" | "inline-flex") && col;
            s.x.grid = matches!(d, "grid" | "inline-grid");
            s.x.inline_block = matches!(d, "inline-block" | "inline-table");
        }
        None => {}
    }
    s.bg_img = bgp.url.map(|url| BgImage { url, size: bgp.size, pos: bgp.pos, repeat: bgp.repeat });
    if let Some(o) = opacity { s.alpha = parent.alpha * o; if o == 0.0 { s.none = true; } }
    if let Some(sh) = s.x.shadow.as_mut() { if sh.color == u32::MAX { sh.color = s.color; } }
    s.mt = (m[0] + p[0]).clamp(0.0, 100.0);
    s.mb = (m[2] + p[2]).clamp(0.0, 100.0);
    (s.pt, s.pb) = (p[0].clamp(0.0, 100.0).min(s.mt), p[2].clamp(0.0, 100.0).min(s.mb));
    s.ml = m[3].clamp(0.0, 400.0);
    s.mr = m[1].clamp(0.0, 400.0);
    s.pl = p[3].clamp(0.0, 400.0);
    s.pr = p[1].clamp(0.0, 400.0);
    (s.auto_l, s.auto_r) = (auto[3], auto[1]);
    if overflow_hidden && matches!(s.width, Some(Len::Px(w)) if w <= 1.0) { s.none = true; }
    for b in s.borders.iter_mut().flatten() { if b.1 == u32::MAX { b.1 = s.color; } }
    (s.pad, s.pad_set) = (p.map(|v| v.clamp(0.0, 100.0)), pad_set);
    s
}

/// Built-in browser defaults, lowest precedence.
pub const UA: &str = "
head,script,style,title,meta,link,template,noscript,option,datalist,object,iframe,canvas,audio,video,[hidden]{display:none}
html,body,div,p,h1,h2,h3,h4,h5,h6,ul,ol,dl,dt,dd,table,tr,section,article,header,footer,nav,main,aside,form,hr,blockquote,figure,figcaption,details,summary,address,pre,fieldset,legend,center,tbody,thead,tfoot,caption{display:block}
li{display:list-item}
td,th{display:inline}
table{text-align:left}
a{color:#0000cc;text-decoration:underline}
h1{font-size:2em;margin:.67em 0;font-weight:bold}
h2{font-size:1.5em;margin:.83em 0;font-weight:bold}
h3{font-size:1.17em;margin:1em 0;font-weight:bold}
h4{font-size:1em;margin:1.33em 0;font-weight:bold}
h5{font-size:.83em;margin:1.67em 0;font-weight:bold}
h6{font-size:.67em;margin:2.33em 0;font-weight:bold}
p,dl,pre,blockquote,figure,fieldset{margin:1em 0}
ul,ol{margin:1em 0;padding-left:40px}
ul ul,ol ol,ul ol,ol ul{margin:0}
blockquote,dd{margin-left:40px}
b,strong,th,dt,summary{font-weight:bold}
i,em,cite,var,dfn,address{font-style:italic}
th,center,caption{text-align:center}
small,sub,sup{font-size:.83em}
sup{vertical-align:super}
sub{vertical-align:sub}
u,ins{text-decoration:underline}
pre,code,kbd,samp,tt{font-family:monospace}
pre{white-space:pre}
hr{border-top:1px solid #999}
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::parse;

    fn style_of(html: &str, css: &str, tag: &str) -> Style {
        let dom = parse(html, true);
        let sheet = Sheet::new(&[UA.into(), css.into()]);
        let id = dom.find(tag).unwrap();
        // walk ancestors so inheritance applies
        let mut chain = vec![];
        let mut p = id;
        while p != 0 { chain.push(p); p = dom.nodes[p].parent; }
        chain.iter().rev().fold(Style::root(), |ps, &i| compute(&dom, &sheet, i, &ps))
    }

    #[test]
    fn cascade_specificity_and_inheritance() {
        let s = style_of("<div id=a class=b><p>x</p></div>", "div{color:red} .b{color:#00ff00} #a{color:blue} p{font-weight:700}", "p");
        assert_eq!(s.color, 0x0000FF);
        assert!(s.bold && s.block);
    }

    #[test]
    fn important_and_inline_and_none() {
        let s = style_of("<p style='color:red'>x</p>", "p{color:blue!important}", "p");
        assert_eq!(s.color, 0x0000FF);
        assert!(style_of("<p hidden>x</p>", "", "p").none);
        assert!(style_of("<p style='display:none'>x</p>", "", "p").none);
        let s = style_of("<p style='position:fixed;top:0'>x</p>", "", "p");
        assert!(!s.none && s.x.pos == 3, "fixed boxes are kept and flagged");
        assert!(!style_of("<p style='position:absolute'>x</p>", "", "p").none);
        assert!(style_of("<a style='position:absolute;width:1px;height:1px;overflow:hidden'>x</a>", "", "a").none);
        assert!(style_of("<a style='clip: rect(0, 0, 0, 0)'>x</a>", "", "a").none);
    }

    #[test]
    fn combinators_and_attrs() {
        let css = "ul > li{color:red} div p{color:blue} a[href^=http]{color:green} li:first-child{font-weight:bold} :not(pre) > code{color:#123456} pre code{color:#654321}";
        assert_eq!(style_of("<p><code>x</code></p>", css, "code").color, 0x123456);
        assert_eq!(style_of("<pre><code>x</code></pre>", css, "code").color, 0x654321);
        assert_eq!(style_of("<ul><li>x</li></ul>", css, "li").color, 0xFF0000);
        assert!(style_of("<ul><li>x</li></ul>", css, "li").bold);
        assert_eq!(style_of("<div><section><p>x</p></section></div>", css, "p").color, 0x0000FF);
        assert_eq!(style_of("<a href='http://x'>x</a>", css, "a").color, 0x008000);
    }

    #[test]
    fn units_media_and_margins() {
        let s = style_of("<p>x</p>", "p{font-size:20px;margin:0 auto 1em 2em}", "p");
        assert_eq!((s.size, s.mt, s.mb, s.ml), (20.0, 0.0, 20.0, 40.0));
        let s = style_of("<div>x</div>", "div{margin:0 auto;max-width:600px;width:50%;padding:4px 8px}", "div");
        assert_eq!((s.auto_l, s.auto_r, s.max_width, s.width, s.pl, s.pr, s.mt), (true, true, Some(Len::Px(600.0)), Some(Len::Pct(50.0)), 8.0, 8.0, 4.0));
        let s = style_of("<p>x</p>", "@media (max-width:600px){p{color:red}} @media print{p{color:red}} @media (min-width:600px){p{color:#010203}}", "p");
        assert_eq!(s.color, 0x010203);
        assert_eq!(parse_color("rgb(1, 2, 3)"), Some(0x010203));
        assert_eq!(parse_color("#abc"), Some(0xAABBCC));
    }

    fn style_at(html: &str, css: &str, tag: &str, n: usize) -> Style {
        let dom = parse(html, true);
        let sheet = Sheet::new(&[UA.into(), css.into()]);
        let id = dom.descendants().into_iter().filter(|&i| dom.nodes[i].tag == tag).nth(n).unwrap();
        let mut chain = vec![];
        let mut p = id;
        while p != 0 { chain.push(p); p = dom.nodes[p].parent; }
        chain.iter().rev().fold(Style::root(), |ps, &i| compute(&dom, &sheet, i, &ps))
    }

    #[test]
    fn custom_properties_and_calc() {
        let css = ":root{--c:#ff0000;--w:200px} p{color:var(--c);width:calc(100% - var(--w)/2);margin-left:var(--m,10px)}";
        let s = style_of("<p>x</p>", css, "p");
        assert_eq!((s.color, s.width, s.ml), (0xFF0000, Some(Len::Mix(100.0, -100.0)), 10.0));
        assert_eq!(style_of("<p>x</p>", "p{width:min(300px,50vw)}", "p").width, Some(Len::Px(300.0)));
        assert_eq!(style_of("<p>x</p>", "p{width:clamp(10px,2em,18px)}", "p").width, Some(Len::Px(18.0)));
        assert_eq!(style_of("<p>x</p>", "p{color:var(--nope)}", "p").color, 0x111111); // undefined without fallback: ignored
        assert_eq!(style_of("<div style='--k:#00ff00'><p>x</p></div>", "p{color:var(--k)}", "p").color, 0x00FF00); // inherited
    }

    #[test]
    fn gradients_fall_back_to_the_first_stop() {
        assert_eq!(style_of("<p>x</p>", "p{background:linear-gradient(to right, #ff0000 0%, #0000ff 100%)}", "p").bg, Some(0xFF0000));
        assert_eq!(style_of("<p>x</p>", "p{background-image:radial-gradient(circle, rgb(1, 2, 3), white)}", "p").bg, Some(0x010203));
        assert_eq!(style_of("<p>x</p>", "p{background:url(a.png)}", "p").bg, None);
    }

    #[test]
    fn alpha_opacity_gradients_shadows_and_grid_values() {
        let s = style_of("<p>x</p>", "p{background:rgba(0,0,0,.5);opacity:.5;z-index:3;letter-spacing:2px;vertical-align:super}", "p");
        assert_eq!((s.bg, s.bg_a, s.alpha, s.x.z, s.ls), (Some(0), 0.5, 0.5, 3, 2.0));
        assert!(s.x.va > 0.2);
        assert_eq!(style_of("<p>x</p>", "p{background:#ff000080}", "p").bg_a, 128.0 / 255.0);
        let g = style_of("<p>x</p>", "p{background:linear-gradient(to right, #ff0000, #0000ff 50%, #00ff00)}", "p").bg_grad.unwrap();
        assert_eq!((g.angle, g.stops.len(), g.stops[1].0), (90.0, 3, 0.5));
        let g = style_of("<p>x</p>", "p{background-image:linear-gradient(45deg, red 0%, blue 100%)}", "p").bg_grad.unwrap();
        assert_eq!(g.angle, 45.0);
        let sh = style_of("<p>x</p>", "p{color:#112233;box-shadow:2px 4px 8px 1px rgba(0,0,0,.25), 0 0 1px red}", "p").x.shadow.unwrap();
        assert_eq!((sh.dx, sh.dy, sh.blur, sh.spread, sh.a), (2.0, 4.0, 8.0, 1.0, 0.25));
        assert_eq!(style_of("<p>x</p>", "p{color:#112233;box-shadow:0 1px 2px}", "p").x.shadow.unwrap().color, 0x112233);
        let x = style_of("<div>x</div>", "div{display:grid;grid-template-columns:100px 1fr repeat(2, 20%);gap:5px 9px}", "div").x;
        assert!(x.grid);
        assert_eq!(x.tracks, vec![Track::Px(100.0), Track::Fr(1.0), Track::Pct(20.0), Track::Pct(20.0)]);
        assert_eq!((x.gap, x.row_gap), (9.0, Some(5.0)));
        let x = style_of("<div>x</div>", "div{display:grid;grid-template-columns:repeat(auto-fill, minmax(120px, 1fr))}", "div").x;
        assert_eq!(x.auto_min, Some(120.0));
        assert!(style_of("<div>x</div>", "div{display:flex;flex-direction:column}", "div").x.flex_col);
        assert_eq!(style_of("<p>x</p>", "p{counter-reset:a 2 b;counter-increment:a}", "p").x.counter_reset, vec![("a".to_string(), 2), ("b".to_string(), 0)]);
    }

    #[test]
    fn radial_gradients_background_layers_and_percent_heights_parse() {
        let g = style_of("<p>x</p>", "p{background:radial-gradient(circle at 20% 80%, #ff0000, #0000ff)}", "p").bg_grad.unwrap();
        assert!(g.radial && g.circle && g.at == (0.2, 0.8) && g.stops.len() == 2);
        let g = style_of("<p>x</p>", "p{background:radial-gradient(#fff, #000)}", "p").bg_grad.unwrap();
        assert!(g.radial && !g.circle && g.at == (0.5, 0.5));
        let b = style_of("<p>x</p>", "p{background:#eee url(a.png) no-repeat right 10px / cover}", "p");
        let i = b.bg_img.unwrap();
        assert_eq!((i.url.as_str(), i.size, i.repeat, i.pos), ("a.png", BgSize::Cover, (false, false), [Len::Pct(100.0), Len::Px(10.0)]));
        assert_eq!(b.bg, Some(0xEEEEEE));
        let i = style_of("<p>x</p>", "p{background-image:url(\"b.png\");background-size:20px auto;background-position:center;background-repeat:repeat-x}", "p").bg_img.unwrap();
        assert_eq!((i.url.as_str(), i.size, i.repeat, i.pos), ("b.png", BgSize::Fixed(Some(Len::Px(20.0)), None), (true, false), [Len::Pct(50.0), Len::Pct(50.0)]));
        assert!(style_of("<p>x</p>", "p{background:url(a.png);background:#fff}", "p").bg_img.is_none(), "a later shorthand resets the image");
        assert_eq!(style_of("<p>x</p>", "p{height:50%}", "p").x.h_pct, Some((50.0, true)));
        assert_eq!(style_of("<p>x</p>", "p{min-height:25%}", "p").x.h_pct, Some((25.0, false)));
        let x = style_of("<p>x</p>", "p{position:sticky;top:5px;vertical-align:middle}", "p").x;
        assert!(x.sticky && x.pos == 1 && x.va_kind == 1);
        assert!(has_state_rules("a:hover{color:red}") && !has_state_rules("a{color:red}"));
    }

    fn has_state_rules(css: &str) -> bool { Sheet::new(&[css.into()]).has_state }

    #[test]
    fn structural_selectors() {
        let css = "li:nth-child(2n+1){color:#010101} li:nth-child(2){color:#020202} li:last-of-type{font-weight:700} :is(h1,h2){color:#030303}";
        let html = "<ul><li>a</li><li>b</li><li>c</li></ul><h2>t</h2>";
        let c: Vec<u32> = (0..3).map(|n| style_at(html, css, "li", n).color).collect();
        assert_eq!(c, vec![0x010101, 0x020202, 0x010101]);
        assert!(!style_at(html, css, "li", 1).bold && style_at(html, css, "li", 2).bold);
        assert_eq!(style_at(html, css, "h2", 0).color, 0x030303);
        assert_eq!(style_at("<input checked>", "input:checked{color:red}", "input", 0).color, 0xFF0000);
    }
}
