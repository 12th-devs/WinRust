// Flow layout: items -> positioned text/pictures/controls plus background rectangles.
// Blocks narrow the line box (margins, padding, borders, width/max-width, auto-centering); tables get real columns;
// floats sit at the left/right edge while following lines wrap around them.
// Also: inline-block atoms, row flexbox (grow/shrink/basis/wrap/gap/justify/align), relative/absolute positioning.
// shortcut: `bottom` of the page-level containing block is
// ignored, z-index is a paint layer per positioned box (no full stacking contexts), flex stretch = start; box insets are not counted in column min/max widths;
// a float that does not fit beside earlier floats drops below them instead of splitting lines.
use crate::{css::{BgSize, Grad, Len, Shadow, Track}, gfx::Fonts, render::{AbsSpec, BgPaint, FlexSpec, FloatSpec, GridSpec, Item, Table}};
use std::{cell::Cell, sync::Arc};

pub const MARGIN: f32 = 16.0;

/// Layer of boxes that stay in the viewport while the page scrolls (`position: fixed`); painted last, at screen coordinates.
pub const FIXED_Z: i64 = 16383 * ZK * ZK * ZK;

// A paint key is up to four base-32768 digits, outermost stacking context first, so an element inside a context with z-index 5
// can never paint above a sibling context with z-index 6 however large its own z-index is. In-flow content has digit 0,
// a positioned box 4*z+2 (auto sits above in-flow text, negative below it), `fixed` adds the digit 16383 on top.
const ZK: i64 = 32768;

fn zdigit(z: i32) -> i64 { 4 * z.clamp(-4000, 4000) as i64 + 2 }

/// Puts an item with key `q` inside a stacking context whose own digit is `d`: its digits move one place down, `d` becomes the top one.
fn zwrap(q: i64, d: i64) -> i64 { d * ZK * ZK * ZK + (q + ZK / 2).div_euclid(ZK) }

// shortcut: the viewport height for `position: fixed` offsets is a thread-local set by `layout_vh`; pass it down if layout ever runs on several threads at once
thread_local! { static VH: Cell<f32> = const { Cell::new(720.0) }; static GRP: Cell<u32> = const { Cell::new(0) }; }

fn next_grp() -> u32 { GRP.with(|g| { g.set(g.get() + 1); g.get() }) }

/// Text run, picture or form control placed on a line. For pictures and controls `size` is the height.
pub struct Placed {
    pub x: f32, pub base: f32, pub size: f32, pub w: f32, pub text: String, pub link: Option<usize>,
    pub color: u32, pub face: u16, pub ul: bool, pub img: Option<usize>, pub field: Option<usize>,
    pub lh: f32,
    pub z: i64,   // paint key (see `zwrap`)
    pub lim: f32, // sticky: how far the box may travel below its pin point (NAN = unlimited; negative = still open while laying out)
    pub grp: u32, // scroll container this item scrolls with (0 = none)
    pub ls: f32,  // letter-spacing in device px
    pub dy: f32,  // baseline shift up in device px (vertical-align: super/sub)
    pub st: f32,  // scroll offset (device px) from which this item is pinned to the viewport (position: fixed = 0, sticky); NAN = never
    pub node: usize, // DOM node the item belongs to (0 = none)
    /// inline-block content (placed text/rects relative to the atom's top-left), flattened into the output when the line is flushed
    pub blk: Option<Box<(Vec<Placed>, Vec<Rect>)>>,
}

/// Filled rectangle; `r` > 0 rounds the corners, `ring` > 0 draws only an outline of that thickness.
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub color: u32, pub r: f32, pub ring: f32, pub node: usize, pub z: i64, pub a: u8, pub grad: Option<Arc<Grad>>, pub pic: Option<Pic>, pub st: f32,
    pub lim: f32, pub grp: u32,
    /// a scroll container's box (invisible): `grp` is its id and this the height of its content; NAN for every other rectangle
    pub scroll: f32 }

/// A background image placed inside its rectangle: tile size and offset in device px, repeat per axis.
#[derive(Clone, Copy)]
pub struct Pic { pub img: usize, pub w: f32, pub h: f32, pub ox: f32, pub oy: f32, pub rx: bool, pub ry: bool }

struct Out { placed: Vec<Placed>, rects: Vec<Rect>, pw: f32 }
impl Out { fn new(pw: f32) -> Out { Out { placed: vec![], rects: vec![], pw } } }

struct Region { node: usize, radius: f32, min_h: f32, pt: f32, pb: f32, bb: bool, scroll: bool, bgimg: Option<Arc<BgPaint>>, r0: usize, p0: usize, a: f32, clip: bool, shadow: Option<Shadow>, grad: Option<Arc<Grad>>, bg: Option<u32>, top: Option<f32>, x0: f32, x1: f32, saved: (f32, f32), borders: [Option<(f32, u32)>; 4], bw: [f32; 4] }

/// An active float: text must stay clear of `edge` until `bottom`.
struct Fl { right_side: bool, edge: f32, bottom: f32 }

fn px(size: f32, sc: f32) -> f32 { (size * sc).round().max(1.0) }

fn resolve(l: Len, avail: f32, sc: f32) -> f32 {
    match l { Len::Px(p) => p * sc, Len::Pct(p) => p / 100.0 * avail, Len::Mix(pc, p) => pc / 100.0 * avail + p * sc }
}

pub fn layout(items: &[Item], width: f32, sc: f32, f: &mut Fonts) -> (Vec<Placed>, Vec<Rect>, f32) { layout_vh(items, width, sc, 720.0 * sc, f) }

/// Like `layout`, with the viewport height (device px) that `position: fixed` offsets from the bottom refer to.
/// Output is sorted by paint layer (z, stable), so within one layer document order is kept.
pub fn layout_vh(items: &[Item], width: f32, sc: f32, vh: f32, f: &mut Fonts) -> (Vec<Placed>, Vec<Rect>, f32) {
    VH.with(|v| v.set(vh));
    GRP.with(|g| g.set(0));
    let m = MARGIN * sc;
    let mut out = Out::new(width);
    let end = block(items, m, width - m, m, sc, f, &mut out);
    out.placed.sort_by_key(|p| p.z);
    out.rects.sort_by_key(|r| r.z);
    (out.placed, out.rects, end + m)
}

/// Distinct paint keys in ascending order (always includes 0) with the ranges of `rects` and `placed` that carry each
/// (both are sorted by key): paint each layer's rectangles, then its text.
pub fn layers(placed: &[Placed], rects: &[Rect]) -> Vec<(i64, std::ops::Range<usize>, std::ops::Range<usize>)> {
    let mut z: Vec<i64> = placed.iter().map(|p| p.z).chain(rects.iter().map(|r| r.z)).chain([0]).collect();
    z.sort_unstable();
    z.dedup();
    z.into_iter().map(|k| (k, rects.partition_point(|r| r.z < k)..rects.partition_point(|r| r.z <= k), placed.partition_point(|p| p.z < k)..placed.partition_point(|p| p.z <= k))).collect()
}

/// A scroll container found in a layout: where its box is, and how tall its content is (device px).
#[derive(Clone, Copy)]
pub struct Group { pub node: usize, pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub content_h: f32 }

pub fn groups(rects: &[Rect]) -> std::collections::HashMap<u32, Group> {
    rects.iter().filter(|r| r.scroll.is_finite()).map(|r| (r.grp, Group { node: r.node, x: r.x, y: r.y, w: r.w, h: r.h, content_h: r.scroll })).collect()
}

/// Tags the content of a scroll container (everything laid out since `p0`/`r0` that is not already in an inner one).
fn mark_group(out: &mut Out, p0: usize, r0: usize, id: u32) {
    let (p0, r0) = (p0.min(out.placed.len()), r0.min(out.rects.len()));
    for p in &mut out.placed[p0..] { if p.grp == 0 { p.grp = id; } }
    for r in &mut out.rects[r0..] { if r.grp == 0 && r.scroll.is_nan() { r.grp = id; } }
}

/// Sticky boxes inside a container that ends at `bottom` may travel only that far: converts their open `lim` (minus the box's bottom edge).
fn close_lims(out: &mut Out, p0: usize, r0: usize, bottom: f32) {
    let (p0, r0) = (p0.min(out.placed.len()), r0.min(out.rects.len()));
    for p in &mut out.placed[p0..] { if p.lim < 0.0 { p.lim = (bottom + p.lim).max(0.0); } }
    for r in &mut out.rects[r0..] { if r.lim < 0.0 { r.lim = (bottom + r.lim).max(0.0); } }
}

/// Translucent layers that fake a blurred `box-shadow`: the outermost is the largest and the faintest.
fn shadow_rects(sh: &Shadow, x: f32, y: f32, w: f32, h: f32, radius: f32) -> Vec<Rect> {
    let n = ((sh.blur / 2.0).ceil() as i32).clamp(1, 8);
    (0..n).map(|i| {
        let grow = sh.spread + sh.blur / 2.0 - sh.blur * i as f32 / n as f32;
        Rect { x: x + sh.dx - grow, y: y + sh.dy - grow, w: (w + 2.0 * grow).max(0.0), h: (h + 2.0 * grow).max(0.0), color: sh.color, r: if radius > 0.0 { radius + grow.max(0.0) } else { 0.0 }, ring: 0.0, node: 0, z: 0, a: (sh.a / n as f32 * 255.0).round().clamp(1.0, 255.0) as u8, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN }
    }).collect()
}

/// `overflow: hidden` with a fixed height: drops text that starts below `bottom` and trims rectangles to it.
fn clip_to(out: &mut Out, p0: usize, r0: usize, bottom: f32) {
    let p0 = p0.min(out.placed.len());
    let tail: Vec<Placed> = out.placed.drain(p0..).filter(|p| p.base <= bottom + 0.5).collect();
    out.placed.extend(tail);
    let r0 = r0.min(out.rects.len());
    let tail: Vec<Rect> = out.rects.drain(r0..).filter(|q| q.y < bottom).map(|mut q| { q.h = q.h.min(bottom - q.y); q }).collect();
    out.rects.extend(tail);
}

/// Leading and trailing vertical margins (device px) of a flex or grid item's list.
fn edge_gaps(items: &[Item], sc: f32) -> (f32, f32) {
    let g = |i: Option<&Item>| if let Some(Item::Break(g)) = i { g * sc } else { 0.0 };
    (g(items.first()), g(items.last()))
}

/// Tile size and offset of a background image inside a `w` x `h` box (device px).
fn resolve_pic(bp: &BgPaint, w: f32, h: f32, sc: f32) -> Pic {
    let (nw, nh) = ((bp.nw * sc).max(1.0), (bp.nh * sc).max(1.0));
    let (tw, th) = match bp.size {
        BgSize::Auto => (nw, nh),
        BgSize::Cover => { let k = (w / nw).max(h / nh); (nw * k, nh * k) }
        BgSize::Contain => { let k = (w / nw).min(h / nh); (nw * k, nh * k) }
        BgSize::Fixed(a, b) => match (a.map(|l| resolve(l, w, sc)), b.map(|l| resolve(l, h, sc))) {
            (Some(x), Some(y)) => (x, y),
            (Some(x), None) => (x, x * nh / nw),
            (None, Some(y)) => (y * nw / nh, y),
            _ => (nw, nh),
        },
    };
    let (tw, th) = (tw.max(1.0), th.max(1.0));
    let at = |l: Len, room: f32, whole: f32| if let Len::Pct(p) = l { (whole - room) * p / 100.0 } else { resolve(l, whole, sc) };
    Pic { img: bp.img, w: tw, h: th, ox: at(bp.pos[0], tw, w), oy: at(bp.pos[1], th, h), rx: bp.repeat.0, ry: bp.repeat.1 }
}

fn rtl_char(c: char) -> bool { matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x200F | 0x202B | 0x202E | 0x2067 | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF) }

fn mirrored(c: char) -> char { match c { '(' => ')', ')' => '(', '[' => ']', ']' => '[', '{' => '}', '}' => '{', '<' => '>', '>' => '<', '\u{ab}' => '\u{bb}', '\u{bb}' => '\u{ab}', c => c } }

/// UAX #9 on one line of words (a picture or control is one neutral object). Words are reordered visually (rule L2 on word levels),
/// words at an odd level get mirrored brackets (shaped scripts are mirrored by the shaper already); returns true for an RTL paragraph.
fn bidi_reorder(line: &mut Vec<Placed>) -> bool {
    use unicode_bidi::{BidiInfo, Level};
    let (mut text, mut spans) = (String::new(), Vec::with_capacity(line.len()));
    for (i, p) in line.iter().enumerate() {
        if i > 0 { text.push(' '); }
        let a = text.len();
        if p.text.is_empty() { text.push('\u{FFFC}'); } else { text.push_str(&p.text); }
        spans.push(a..text.len());
    }
    let info = BidiInfo::new(&text, None);
    let Some(para) = info.paragraphs.first() else { return false };
    let base = para.level.is_rtl();
    let levels: Vec<Level> = spans.iter().map(|s| info.levels[s.clone()].iter().copied().max().unwrap_or(para.level)).collect();
    if levels.iter().all(|l| *l == Level::ltr()) { return false; }
    let n = line.len();
    let x0 = line.iter().fold(f32::MAX, |m, p| m.min(p.x));
    let gaps: Vec<f32> = (0..n).map(|i| if i == 0 { 0.0 } else { (line[i].x - (line[i - 1].x + line[i - 1].w)).max(0.0) }).collect();
    let space = gaps.iter().copied().find(|g| *g > 0.0).unwrap_or(0.0);
    let order = BidiInfo::reorder_visual(&levels);
    let (mut x, mut new_x) = (x0, vec![0f32; n]);
    for (k, &i) in order.iter().enumerate() {
        if k > 0 { let prev = order[k - 1]; x += if i == prev + 1 { gaps[i] } else if prev == i + 1 { gaps[prev] } else { space }; }
        new_x[i] = x;
        x += line[i].w;
    }
    for (i, p) in line.iter_mut().enumerate() {
        p.x = new_x[i];
        if levels[i].is_rtl() && !crate::gfx::complex(&p.text) && p.text.chars().any(|c| mirrored(c) != c) { p.text = p.text.chars().map(mirrored).collect(); }
    }
    base
}

/// Moves the finished line down to the baseline `y + tallest` and applies text-align inside the line box.
fn flush_line(line: &mut Vec<Placed>, out: &mut Out, y: &mut f32, right: f32, align: u8) {
    if line.is_empty() { return; }
    let h = line.iter().fold(0f32, |m, p| m.max(p.size));
    let adv = line.iter().fold(0f32, |m, p| m.max(p.size * p.lh));
    let lead = (adv - h * 1.35) / 2.0; // half-leading when line-height differs from the 1.35 default
    // Bidi: UAX #9 over the words of the line decides the paragraph direction and each word's embedding level.
    let rtl = if line.iter().any(|p| p.text.chars().any(rtl_char)) { bidi_reorder(line) } else { false };
    let last = line.iter().max_by(|a, b| (a.x + a.w).total_cmp(&(b.x + b.w))).map_or(0.0, |p| p.x + p.w);
    let free = (right - last).max(0.0);
    let shift = match align { 1 => free / 2.0, 2 => free, _ if rtl => free, _ => 0.0 };
    for mut p in line.drain(..) {
        p.base = *y + h + lead - p.dy;
        p.x += shift;
        if let Some(b) = p.blk.take() {
            let (dx, dy) = (p.x, p.base - p.size);
            out.placed.extend(b.0.into_iter().map(|mut q| { q.x += dx; q.base += dy; q }));
            out.rects.extend(b.1.into_iter().map(|mut r| { r.x += dx; r.y += dy; r }));
        } else { out.placed.push(p); }
    }
    *y += adv;
}

/// Line box [l, r] at height `y` after subtracting active floats.
fn eff(floats: &[Fl], y: f32, left: f32, right: f32) -> (f32, f32) {
    let (mut l, mut r) = (left, right);
    for f in floats.iter().filter(|f| f.bottom > y) {
        if f.right_side { r = r.min(f.edge); } else { l = l.max(f.edge); }
    }
    (l, r.max(l + 20.0))
}

fn border_rects(rects: &mut Vec<Rect>, r: &Region, y_end: f32) -> f32 {
    let Some(top) = r.top else { return 0.0 };
    let [bt, br, bb, bl] = r.bw;
    let (w, h) = (r.x1 - r.x0, y_end - top);
    if let (true, [Some(a), Some(b), Some(c), Some(d)]) = (r.radius > 0.0, r.borders) {
        if a == b && b == c && c == d { // one rounded ring instead of four straight sides
            rects.push(Rect { x: r.x0, y: top - bt, w, h: h + bt + bb, color: a.1, r: r.radius, ring: bt, node: r.node, z: 0, a: 255, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN });
            return bb;
        }
    }
    let side = |rects: &mut Vec<Rect>, b: Option<(f32, u32)>, x: f32, y: f32, w: f32, h: f32| if let Some((_, c)) = b { rects.push(Rect { x, y, w, h, color: c, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN }); };
    side(rects, r.borders[0], r.x0, top - bt, w, bt);
    side(rects, r.borders[2], r.x0, y_end, w, bb);
    side(rects, r.borders[3], r.x0, top - bt, bl, h + bt + bb);
    side(rects, r.borders[1], r.x1 - br, top - bt, br, h + bt + bb);
    bb
}

/// Lays `items` out between `left0` and `right0` starting at `y0`; returns the y after the last line.
fn block<'a>(items: &'a [Item], left0: f32, right0: f32, y0: f32, sc: f32, f: &mut Fonts, out: &mut Out) -> f32 {
    let (mut left, mut right) = (left0, right0);
    let mut line: Vec<Placed> = vec![];
    let (mut x, mut y, mut gap, mut align) = (left, y0, 0f32, 0u8);
    let (mut cur_l, mut cur_r) = (left, right);
    let (sp0, sr0) = (out.placed.len(), out.rects.len());
    let mut soft_last = false; // the last word on the line ended with a soft hyphen (shown only when the line breaks there)
    let mut regions: Vec<Region> = vec![];
    let mut floats: Vec<Fl> = vec![];
    let mut pos: Vec<Pos<'a>> = vec![];
    macro_rules! flush { () => { flush_line(&mut line, out, &mut y, cur_r, align) }; }
    macro_rules! mark_tops { ($y:expr) => { for r in regions.iter_mut().filter(|r| r.top.is_none()) { r.top = Some($y); } for p in pos.iter_mut().filter(|p| p.top.is_none()) { p.top = Some($y); } }; }
    for it in items {
        match it {
            Item::Break(g) => { flush!(); gap = gap.max(g * sc); }
            Item::Box(b) => {
                flush!();
                let avail = right - left;
                let bw = b.borders.map(|o| o.map_or(0.0, |(w, _)| (w * sc).round().max(1.0)));
                let (inl, inr, pl, pr) = (b.ml * sc, b.mr * sc, b.pl * sc + bw[3], b.pr * sc + bw[1]);
                let inner = (avail - inl - inr - pl - pr).max(20.0);
                let mut w = inner;
                let trim = if b.bb { pl + pr } else { 0.0 }; // border-box: width includes padding and borders
                if let Some(wd) = b.width { w = (resolve(wd, avail, sc) - trim).max(0.0); }
                if let Some(mw) = b.max_width { w = w.min((resolve(mw, avail, sc) - trim).max(0.0)); }
                w = w.clamp(20.0, inner);
                let extra = (inner - w).max(0.0);
                let off = if b.auto_l && b.auto_r { extra / 2.0 } else if b.auto_l { extra } else { 0.0 };
                let bx0 = left + inl + off;
                let starts_now = b.min_h > 0.0 || b.pt > 0.0 || b.pb > 0.0 || b.shadow.is_some() || b.bgimg.is_some();
                if starts_now { // a fixed-height or padded box starts here even when it holds no flow content
                    if gap > 0.0 && y > y0 + 0.5 { y += gap; }
                    gap = 0.0;
                }
                regions.push(Region { node: b.node, radius: b.radius * sc, min_h: b.min_h * sc, pt: b.pt * sc, pb: b.pb * sc, bb: b.bb, scroll: b.scroll, bgimg: b.bgimg.clone(), r0: out.rects.len(), p0: out.placed.len(), a: b.bg_a, clip: b.clip, shadow: b.shadow, grad: b.grad.clone(), bg: b.bg, top: starts_now.then_some(y), x0: bx0, x1: bx0 + pl + w + pr, saved: (left, right), borders: b.borders, bw });
                left = bx0 + pl;
                right = left + w;
                (cur_l, cur_r) = (left, right);
                y += b.pt * sc;
            }
            Item::BoxEnd => {
                flush!();
                if let Some(mut r) = regions.pop() {
                    y += r.pb;
                    let mut content_h = f32::NAN; // overflow: auto|scroll with a fixed height: the box keeps its height, the content scrolls inside
                    if r.min_h > 0.0 {
                        let top = *r.top.get_or_insert(y);
                        let need = top + if r.bb { r.min_h } else { r.pt + r.min_h + r.pb };
                        if r.scroll { content_h = (y - top).max(0.0); }
                        y = if r.clip || r.scroll { need } else { y.max(need) };
                    }
                    if let (true, Some(top)) = (r.clip, r.top) { clip_to(out, r.p0, r.r0, y.max(top)); }
                    close_lims(out, r.p0, r.r0, y);
                    if let Some(top) = r.top {
                        let at = r.r0.min(out.rects.len());
                        let mut under: Vec<Rect> = vec![];
                        if let Some(sh) = &r.shadow { under.extend(shadow_rects(sh, r.x0, top, r.x1 - r.x0, y - top, r.radius)); }
                        if let Some(color) = r.bg { under.push(Rect { x: r.x0, y: top, w: r.x1 - r.x0, h: y - top, color, r: r.radius, ring: 0.0, node: r.node, z: 0, a: (r.a * 255.0).round().clamp(0.0, 255.0) as u8, grad: r.grad.clone(), pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN }); }
                        if let Some(bp) = &r.bgimg { under.push(Rect { x: r.x0, y: top, w: r.x1 - r.x0, h: y - top, color: 0, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: Some(resolve_pic(bp, r.x1 - r.x0, y - top, sc)), st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN }); }
                        let marker = |grp: u32, scroll: f32| Rect { x: r.x0, y: top, w: r.x1 - r.x0, h: y - top, color: 0, r: 0.0, ring: 0.0, node: r.node, z: 0, a: 0, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp, scroll };
                        if content_h.is_finite() {
                            let id = next_grp();
                            mark_group(out, r.p0, r.r0, id);
                            under.push(marker(id, content_h));
                        } else if r.bg.is_none() && r.node != 0 { under.push(marker(0, f32::NAN)); } // box geometry for hit-testing and getBoundingClientRect
                        out.rects.splice(at..at, under);
                    }
                    y += border_rects(&mut out.rects, &r, y);
                    (left, right) = r.saved;
                    (cur_l, cur_r) = (left, right);
                }
            }
            Item::Table(t) => {
                flush!();
                if gap > 0.0 && y > y0 + 0.5 { y += gap; }
                gap = 0.0;
                mark_tops!(y);
                let (l, r) = eff(&floats, y, left, right);
                y = table(t, l, r, y, sc, f, out);
            }
            Item::Float(fl) => {
                flush!();
                if gap > 0.0 && y > y0 + 0.5 { y += gap; }
                gap = 0.0;
                mark_tops!(y);
                let avail = right - left;
                let w = spec_width(fl, avail, sc, f);
                let mut fy = y;
                loop {
                    let (l2, r2) = eff(&floats, fy, left, right);
                    let active_bottom = floats.iter().filter(|f| f.bottom > fy).map(|f| f.bottom).fold(f32::MAX, f32::min);
                    if w <= r2 - l2 + 0.5 || active_bottom == f32::MAX { break; }
                    fy = active_bottom;
                }
                let (l2, r2) = eff(&floats, fy, left, right);
                let fx = if fl.right { r2 - w } else { l2 };
                let end = block(&fl.items, fx, fx + w, fy, sc, f, out);
                let spacing = 8.0 * sc;
                floats.push(Fl { right_side: fl.right, edge: if fl.right { fx - spacing } else { fx + w + spacing }, bottom: end });
            }
            Item::Clear(bits) => {
                flush!();
                for fl in floats.iter().filter(|fl| (bits & 1 != 0 && !fl.right_side) || (bits & 2 != 0 && fl.right_side)) { y = y.max(fl.bottom); }
            }
            Item::Rule(color) => {
                flush!();
                if gap > 0.0 && y > y0 + 0.5 { y += gap; }
                gap = 0.0;
                mark_tops!(y);
                let (l, r) = eff(&floats, y, left, right);
                let h = sc.round().max(1.0);
                out.rects.push(Rect { x: l, y, w: r - l, h, color: *color, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN });
                y += h + 2.0 * sc;
            }
            Item::Flex(sp) => {
                flush!();
                if gap > 0.0 && y > y0 + 0.5 { y += gap; }
                gap = 0.0;
                mark_tops!(y);
                let (l, r) = eff(&floats, y, left, right);
                y = flex(sp, l, r, y, sc, f, out);
            }
            Item::RelStart(off, z, sticky) => {
                let off = (*off, *z, *sticky);
                flush!();
                if gap > 0.0 && y > y0 + 0.5 { y += gap; }
                gap = 0.0;
                pos.push(Pos { x0: left, x1: right, top: Some(y), p0: out.placed.len(), r0: out.rects.len(), off: off.0, z: off.1, sticky: off.2, pending: vec![] });
            }
            Item::RelEnd => {
                flush!();
                if let Some(p) = pos.pop() { close_pos(p, y, sc, f, out); }
            }
            Item::Abs(a) => if a.fixed {
                let (pw, vh) = (out.pw, VH.with(Cell::get));
                place_abs(a, (0.0, 0.0, pw, Some(vh)), (left, y), sc, f, out);
            } else {
                match pos.last_mut() {
                    Some(p) => p.pending.push((a, left, y)),
                    None => { let pw = out.pw; place_abs(a, (0.0, 0.0, pw, None), (left, y), sc, f, out); }
                }
            },
            Item::Grid(sp) => {
                flush!();
                if gap > 0.0 && y > y0 + 0.5 { y += gap; }
                gap = 0.0;
                mark_tops!(y);
                let (l, r) = eff(&floats, y, left, right);
                y = grid_layout(sp, l, r, y, sc, f, out);
            }
            Item::Word { .. } | Item::Image { .. } | Item::Field { .. } | Item::InlineBlock(_) => {
                let soft = matches!(it, Item::Word { text, .. } if text.ends_with('\u{ad}'));
                let (size, w, space, indent, a, text, color, face, ul, link, img, field, lh, node, blk, ls, dy) = match it {
                    Item::Word { text, size, color, bold, italic, fam, ul, mono, link, space, indent, align, lh, node, ls, dy } => {
                        let size = px(*size, sc);
                        let face = crate::gfx::face(*bold, *italic, *mono, *fam);
                        let ls = ls * sc;
                        let text = text.trim_end_matches('\u{ad}');
                        (size, f.width_f(text, size, face) + ls * text.chars().count() as f32, *space, *indent, *align, text.to_string(), *color, face, *ul, *link, None, None, *lh, *node, None, ls, dy * size)
                    }
                    Item::Image { img, w, h, link, indent, align, node, va, alpha, pct } => {
                        let (mut w, mut h) = (w * sc, h * sc);
                        let avail = cur_r - cur_l - indent * sc;
                        if let (Some(p), true) = (pct, w > 0.0) { let nw = avail * p / 100.0; h *= nw / w; w = nw; }
                        if w > avail && avail > 0.0 { h *= avail / w; w = avail; }
                        // vertical-align: middle centers the picture on the baseline plus half the x-height; top/bottom keep the baseline
                        let dy = if *va == 1 { 4.0 * sc - h / 2.0 } else { 0.0 };
                        (h.max(1.0), w, false, *indent, *align, String::new(), ((alpha * 255.0).round() as u32).clamp(1, 255), 0, false, *link, Some(*img), None, 1.35, *node, None, 0.0, dy)
                    }
                    Item::Field { idx, w, h, indent, align, node } => {
                        let (w, h) = ((w * sc).min(cur_r - cur_l - indent * sc).max(20.0), h * sc);
                        (h, w, true, *indent, *align, String::new(), 0, 0, false, None, None, Some(*idx), 1.35, *node, None, 0.0, 0.0)
                    }
                    Item::InlineBlock(fl) => {
                        let w = spec_width(fl, (right - left).max(20.0), sc, f);
                        let mut tmp = Out::new(out.pw);
                        let h = block(&fl.items, 0.0, w, 0.0, sc, f, &mut tmp).max(1.0);
                        (h, w, true, 0.0, align, String::new(), 0, 0, false, None, None, None, 1.35, 0, Some(Box::new((tmp.placed, tmp.rects))), 0.0, 0.0)
                    }
                    _ => unreachable!(),
                };
                let mut sp = if space && !line.is_empty() { f.width(" ", size) } else { 0.0 };
                if !line.is_empty() && x + sp + w > cur_r {
                    if soft_last { if let Some(l) = line.last_mut() { let hw = f.width("-", l.size); l.text.push('-'); l.w += hw; } }
                    flush!();
                    sp = 0.0;
                }
                if line.is_empty() {
                    if gap > 0.0 && y > y0 + 0.5 { y += gap; }
                    gap = 0.0;
                    (cur_l, cur_r) = eff(&floats, y, left, right);
                    x = cur_l + indent * sc;
                    align = a;
                    mark_tops!(y);
                } else { x += sp; }
                line.push(Placed { x, base: 0.0, size, w, text, link, color, face, ul, img, field, lh, node, blk, z: 0, ls, dy, st: f32::NAN, lim: f32::NAN, grp: 0 });
                soft_last = soft;
                x += w;
            }
        }
    }
    flush!();
    // a box left open by malformed item streams still closes cleanly
    for r in regions.drain(..) {
        if let (Some(color), Some(top)) = (r.bg, r.top) { out.rects.push(Rect { x: r.x0, y: top, w: r.x1 - r.x0, h: y - top, color, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN }); }
    }
    for p in pos.drain(..) { close_pos(p, y, sc, f, out); }
    close_lims(out, sp0, sr0, y);
    // content that is shorter than a float it contains still ends below it, like a cleared container would
    floats.iter().map(|f| f.bottom).fold(y, f32::max)
}

/// (min-content, max-content) width of an item list: longest word / longest unwrapped line.
fn measure(items: &[Item], sc: f32, f: &mut Fonts) -> (f32, f32) {
    let (mut min, mut maxw, mut line, mut first) = (0f32, 0f32, 0f32, true);
    for it in items {
        match it {
            Item::Word { text, size, space, bold, italic, fam, mono, ls, .. } => {
                let s = px(*size, sc);
                let text = text.trim_end_matches('\u{ad}');
                let w = f.width_f(text, s, crate::gfx::face(*bold, *italic, *mono, *fam)) + ls * sc * text.chars().count() as f32;
                line += if *space && !first { f.width(" ", s) } else { 0.0 } + w;
                min = min.max(w);
                first = false;
            }
            Item::Image { w, .. } | Item::Field { w, .. } => { line += w * sc; min = min.max(w * sc); first = false; }
            Item::Break(_) | Item::Rule(_) | Item::Clear(_) => { maxw = maxw.max(line); line = 0.0; first = true; }
            Item::Table(t) => {
                let (cmin, cmax) = col_metrics(t, sc, f);
                let sp = t.spacing * sc * (cmin.len() as f32 + 1.0);
                min = min.max(cmin.iter().sum::<f32>() + sp);
                maxw = maxw.max(line).max(cmax.iter().sum::<f32>() + sp);
                line = 0.0;
                first = true;
            }
            Item::InlineBlock(fl) => {
                let (mn, mx) = measure(&fl.items, sc, f);
                let inset = box_inset(&fl.items, sc);
                line += if first { 0.0 } else { f.width(" ", 16.0 * sc) } + mx + inset;
                min = min.max(mn + inset);
                first = false;
            }
            Item::Flex(sp) => {
                let ms: Vec<(f32, f32)> = sp.items.iter().map(|i| { let (a, b) = measure(&i.items, sc, f); let n = box_inset(&i.items, sc); (a + n, b + n) }).collect();
                let g = sp.gap * sc * ms.len().saturating_sub(1) as f32;
                if sp.col {
                    min = min.max(ms.iter().fold(0f32, |m, x| m.max(x.0)));
                    maxw = maxw.max(line).max(ms.iter().fold(0f32, |m, x| m.max(x.1)));
                    line = 0.0;
                    first = true;
                    continue;
                }
                min = min.max(if sp.wrap { ms.iter().fold(0f32, |m, x| m.max(x.0)) } else { ms.iter().map(|x| x.0).sum::<f32>() + g });
                maxw = maxw.max(line).max(ms.iter().map(|x| x.1).sum::<f32>() + g);
                line = 0.0;
                first = true;
            }
            Item::Grid(sp) => {
                let ms: Vec<(f32, f32)> = sp.items.iter().map(|i| { let (a, b) = measure(&i.items, sc, f); let n = box_inset(&i.items, sc); (a + n, b + n) }).collect();
                min = min.max(ms.iter().fold(0f32, |m, x| m.max(x.0)));
                maxw = maxw.max(line).max(ms.iter().map(|x| x.1).sum::<f32>() + sp.col_gap * sc * ms.len().saturating_sub(1) as f32);
                line = 0.0;
                first = true;
            }
            Item::Box(_) | Item::BoxEnd | Item::Float(_) | Item::Abs(_) | Item::RelStart(..) | Item::RelEnd => {}
        }
    }
    (min, maxw.max(line))
}

/// Horizontal margin + padding of a leading Box item.
fn box_inset(items: &[Item], sc: f32) -> f32 {
    // the Box follows the element's top-margin Break
    items.iter().take_while(|i| matches!(i, Item::Break(_) | Item::Box(_))).find_map(|i| if let Item::Box(b) = i { Some((b.ml + b.mr + b.pl + b.pr) * sc) } else { None }).unwrap_or(0.0)
}

/// Outer width of a float / inline-block: explicit width or shrink-to-fit within `avail`.
fn spec_width(fl: &FloatSpec, avail: f32, sc: f32, f: &mut Fonts) -> f32 {
    let (mn, mx) = measure(&fl.items, sc, f);
    let inset = box_inset(&fl.items, sc);
    let mut w = match fl.width { Some(wd) => resolve(wd, avail, sc), None => (mx + inset).min(avail).max((mn + inset).min(avail)) };
    if let Some(m) = fl.max_width { w = w.min(resolve(m, avail, sc)); }
    w.clamp(20.0, avail.max(20.0))
}

/// Open `position:relative` box: its content range in the output and the absolutes waiting for its final height.
struct Pos<'a> { x0: f32, x1: f32, top: Option<f32>, p0: usize, r0: usize, off: [Option<Len>; 4], z: i32, sticky: bool, pending: Vec<(&'a AbsSpec, f32, f32)> }

fn close_pos(p: Pos, y: f32, sc: f32, f: &mut Fonts, out: &mut Out) {
    let top = p.top.unwrap_or(y);
    for (a, sx, sy) in &p.pending { place_abs(a, (p.x0, top, p.x1, Some(y)), (*sx, *sy), sc, f, out); }
    let w = p.x1 - p.x0;
    let off = |i: usize| p.off[i].map(|l| resolve(l, w, sc));
    let (dx, dy) = if p.sticky { (0.0, 0.0) } else { (off(3).or(off(1).map(|v| -v)).unwrap_or(0.0), off(0).or(off(2).map(|v| -v)).unwrap_or(0.0)) };
    let (p0, r0) = (p.p0.min(out.placed.len()), p.r0.min(out.rects.len()));
    if dx != 0.0 || dy != 0.0 {
        for q in &mut out.placed[p0..] { q.x += dx; q.base += dy; }
        for r in &mut out.rects[r0..] { r.x += dx; r.y += dy; }
    }
    if p.sticky { // pinned by the painter once the page scrolls past where the box would leave the top offset
        let st = top - off(0).unwrap_or(0.0);
        for q in &mut out.placed[p0..] { if q.st.is_nan() { q.st = st; q.lim = -y; } }
        for r in &mut out.rects[r0..] { if r.st.is_nan() { r.st = st; r.lim = -y; } }
    }
    if p.z != 0 { // a stacking context: its whole content moves to the layer of its z-index
        let d = zdigit(p.z);
        for q in &mut out.placed[p0..] { q.z = zwrap(q.z, d); }
        for r in &mut out.rects[r0..] { r.z = zwrap(r.z, d); }
    }
}

/// Places a `position:absolute` box in the containing block (x0, top, x1, bottom); `st` is its static position.
fn place_abs(a: &AbsSpec, cb: (f32, f32, f32, Option<f32>), st: (f32, f32), sc: f32, f: &mut Fonts, out: &mut Out) {
    let (cx0, ct, cx1, cbot) = cb;
    let cw = (cx1 - cx0).max(20.0);
    let off = |i: usize| a.off[i].map(|l| resolve(l, cw, sc));
    let (t, r, b, l) = (off(0), off(1), off(2), off(3));
    let (mn, mx) = measure(&a.items, sc, f);
    let inset = box_inset(&a.items, sc);
    let room = (cw - l.unwrap_or(0.0) - r.unwrap_or(0.0)).max(20.0);
    let mut w = match (a.width, l, r) {
        (Some(wd), _, _) => resolve(wd, cw, sc),
        (None, Some(l), Some(r)) => cw - l - r,
        _ => (mx + inset).max(mn + inset).min(room),
    };
    if let Some(m) = a.max_width { w = w.min(resolve(m, cw, sc)); }
    let w = w.clamp(20.0, cw.max(20.0));
    let x = match (l, r) { (Some(l), _) => cx0 + l, (None, Some(r)) => cx1 - r - w, _ => st.0 };
    let mut tmp = Out::new(out.pw);
    let h = block(&a.items, x, x + w, 0.0, sc, f, &mut tmp);
    let y = match (t, b, cbot) { (Some(t), _, _) => ct + t, (None, Some(b), Some(bot)) => bot - b - h, _ => st.1 };
    // positioned boxes paint above in-flow content, by z-index, as a stacking context of their own; fixed ones also stay in the viewport
    let (d, pin) = (zdigit(a.z), if a.fixed { 0.0 } else { f32::NAN });
    let wrap = |z: i64| { let z = zwrap(z, d); if a.fixed { zwrap(z, 16383) } else { z } };
    out.placed.extend(tmp.placed.into_iter().map(|mut p| { p.base += y; p.z = wrap(p.z); if p.st.is_nan() { p.st = pin; } else if !a.fixed { p.st += y; } p }));
    out.rects.extend(tmp.rects.into_iter().map(|mut r| { r.y += y; r.z = wrap(r.z); if r.st.is_nan() { r.st = pin; } else if !a.fixed { r.st += y; } r }));
}

/// Row flex container: sizes items (basis/width/content, grow, shrink), wraps into lines, then justifies and aligns.
/// Column flex container: items stack with `gap` and their own margins; `align-items` places them across.
/// shortcut: no main-axis sizing (grow/shrink/justify need a container height, which is only a minimum here)
fn flex_col(sp: &FlexSpec, left: f32, right: f32, y0: f32, sc: f32, f: &mut Fonts, out: &mut Out) -> f32 {
    let avail = (right - left).max(1.0);
    let gap = sp.gap * sc;
    let mut y = y0;
    for (k, it) in sp.items.iter().enumerate() {
        let (_, mx) = measure(&it.items, sc, f);
        let inset = box_inset(&it.items, sc);
        let al = if it.align != 0 { it.align } else { sp.align };
        let w = match it.width { Some(wd) => resolve(wd, avail, sc) + inset, None if al == 0 => avail, None => mx + inset }.clamp(1.0, avail);
        let off = if it.auto_l && it.auto_r { (avail - w) / 2.0 } else if it.auto_l { avail - w } else { match al { 2 => (avail - w) / 2.0, 3 => avail - w, _ => 0.0 } };
        let (lead, trail) = edge_gaps(&it.items, sc);
        let end = block(&it.items, left + off, left + off + w, y + lead, sc, f, out);
        y = end + trail + if k + 1 < sp.items.len() { gap } else { 0.0 };
    }
    y
}

/// Grid container: columns from `grid-template-columns` (fr, px, %, repeat, auto-fill), items flow row by row and may span columns.
/// shortcut: rows are as tall as their tallest item; no explicit rows, areas, named lines or dense packing
fn grid_layout(sp: &GridSpec, left: f32, right: f32, y0: f32, sc: f32, f: &mut Fonts, out: &mut Out) -> f32 {
    let avail = (right - left).max(1.0);
    let (cg, rg) = (sp.col_gap * sc, sp.row_gap * sc);
    let mut tracks = sp.tracks.clone();
    if let Some(minw) = sp.auto_min { tracks = vec![Track::Fr(1.0); (((avail + cg) / (minw * sc + cg)).floor() as usize).clamp(1, 24)]; }
    if tracks.is_empty() { tracks.push(Track::Fr(1.0)); }
    let n = tracks.len();
    let fixed: f32 = tracks.iter().map(|t| match t { Track::Px(p) => p * sc, Track::Pct(p) => p / 100.0 * avail, Track::Fr(_) => 0.0 }).sum();
    let frs: f32 = tracks.iter().map(|t| if let Track::Fr(k) = t { *k } else { 0.0 }).sum();
    let free = (avail - fixed - cg * (n as f32 - 1.0)).max(0.0);
    let widths: Vec<f32> = tracks.iter().map(|t| match t { Track::Px(p) => p * sc, Track::Pct(p) => p / 100.0 * avail, Track::Fr(k) => if frs > 0.0 { free * k / frs } else { 0.0 } }).collect();
    let mut rows: Vec<Vec<(usize, usize, usize)>> = vec![vec![]];
    let mut col = 0;
    for (i, it) in sp.items.iter().enumerate() {
        let span = it.span.clamp(1, n);
        if col + span > n { rows.push(vec![]); col = 0; }
        rows.last_mut().unwrap().push((i, col, span));
        col += span;
        if col >= n { rows.push(vec![]); col = 0; }
    }
    rows.retain(|r| !r.is_empty());
    let mut y = y0;
    for (ri, row) in rows.iter().enumerate() {
        let (mut cells, mut rh) = (vec![], 0f32);
        for &(i, col, span) in row {
            let it = &sp.items[i];
            let x = left + widths[..col].iter().sum::<f32>() + cg * col as f32;
            let w = widths[col..col + span].iter().sum::<f32>() + cg * (span as f32 - 1.0);
            let (lead, trail) = edge_gaps(&it.items, sc);
            let mut tmp = Out::new(out.pw);
            let end = block(&it.items, x, x + w, y + lead, sc, f, &mut tmp);
            rh = rh.max(end - y + trail);
            cells.push(tmp);
        }
        for tmp in cells { out.placed.extend(tmp.placed); out.rects.extend(tmp.rects); }
        y += rh + if ri + 1 < rows.len() { rg } else { 0.0 };
    }
    y
}

fn flex(sp: &FlexSpec, left: f32, right: f32, y0: f32, sc: f32, f: &mut Fonts, out: &mut Out) -> f32 {
    if sp.col { return flex_col(sp, left, right, y0, sc, f, out); }
    let avail = (right - left).max(1.0);
    let gap = sp.gap * sc;
    let m: Vec<(f32, f32)> = sp.items.iter().map(|it| {
        let (mn, mx) = measure(&it.items, sc, f);
        let inset = box_inset(&it.items, sc);
        let mut base = match (it.basis, it.width) { (Some(b), _) => resolve(b, avail, sc) + inset, (None, Some(w)) => resolve(w, avail, sc) + inset, _ => mx + inset };
        if let Some(mw) = it.max_width { base = base.min(resolve(mw, avail, sc) + inset); }
        let min = (mn + inset).min(base);
        (base, min)
    }).collect();
    let mut lines: Vec<Vec<usize>> = vec![];
    let (mut cur, mut sum) = (vec![], 0f32);
    for (i, (base, _)) in m.iter().enumerate() {
        if sp.wrap && !cur.is_empty() && sum + gap + base > avail { lines.push(std::mem::take(&mut cur)); sum = 0.0; }
        sum += if cur.is_empty() { 0.0 } else { gap } + base;
        cur.push(i);
    }
    if !cur.is_empty() { lines.push(cur); }
    let mut y = y0;
    for (li, line) in lines.iter().enumerate() {
        let n = line.len() as f32;
        let mut w: Vec<f32> = line.iter().map(|&i| m[i].0).collect();
        let free = avail - w.iter().sum::<f32>() - gap * (n - 1.0);
        if free > 0.0 {
            let tg: f32 = line.iter().map(|&i| sp.items[i].grow).sum();
            if tg > 0.0 { for (k, &i) in line.iter().enumerate() { w[k] += free * sp.items[i].grow / tg; } }
        } else if free < 0.0 {
            let sw: f32 = line.iter().map(|&i| sp.items[i].shrink * m[i].0).sum();
            if sw > 0.0 { for (k, &i) in line.iter().enumerate() { w[k] = (m[i].0 + free * sp.items[i].shrink * m[i].0 / sw).max(m[i].1); } }
        }
        let free = (avail - w.iter().sum::<f32>() - gap * (n - 1.0)).max(0.0);
        let autos: f32 = line.iter().map(|&i| sp.items[i].auto_l as u8 as f32 + sp.items[i].auto_r as u8 as f32).sum();
        let (mut start, mut between) = (0.0, 0.0);
        if autos == 0.0 {
            match sp.justify {
                1 => start = free / 2.0,
                2 => start = free,
                3 if n > 1.0 => between = free / (n - 1.0),
                4 => { between = free / n; start = between / 2.0; }
                5 => { between = free / (n + 1.0); start = between; }
                _ => {}
            }
        }
        let mut x = left + start;
        let mut cells = vec![];
        let mut line_h = 0f32;
        for (k, &i) in line.iter().enumerate() {
            let it = &sp.items[i];
            if autos > 0.0 && it.auto_l { x += free / autos; }
            let mut tmp = Out::new(out.pw);
            let h = block(&it.items, x, x + w[k], y, sc, f, &mut tmp) - y;
            line_h = line_h.max(h);
            cells.push((tmp, h, if it.align != 0 { it.align } else { sp.align }));
            x += w[k] + gap + between + if autos > 0.0 && it.auto_r { free / autos } else { 0.0 };
        }
        for (mut tmp, h, al) in cells {
            let dy = match al { 2 => (line_h - h) / 2.0, 3 => line_h - h, _ => 0.0 };
            for p in &mut tmp.placed { p.base += dy; }
            for r in &mut tmp.rects { r.y += dy; }
            out.placed.extend(tmp.placed);
            out.rects.extend(tmp.rects);
        }
        y += line_h + if li + 1 < lines.len() { gap } else { 0.0 };
    }
    y
}

/// Start column of every cell (cells covered by a rowspan from an earlier row are skipped) and the column count.
fn grid(t: &Table) -> (usize, Vec<Vec<usize>>) {
    let (mut busy, mut n, mut starts) = (Vec::<usize>::new(), 0, vec![]);
    for row in &t.rows {
        let (mut col, mut rs) = (0, vec![]);
        for cell in &row.cells {
            while busy.get(col).is_some_and(|&b| b > 0) { col += 1; }
            let span = if cell.colspan >= 1000 { 1 } else { cell.colspan.max(1) };
            if busy.len() < col + span { busy.resize(col + span, 0); }
            for b in &mut busy[col..col + span] { *b = cell.rowspan.max(1); }
            rs.push(col);
            col += span;
            n = n.max(col);
        }
        for b in &mut busy { *b = b.saturating_sub(1); }
        starts.push(rs);
    }
    (n, starts)
}

/// Per-column (min, max) widths including cell padding.
fn col_metrics(t: &Table, sc: f32, f: &mut Fonts) -> (Vec<f32>, Vec<f32>) {
    let (n, starts) = grid(t);
    let (mut cmin, mut cmax) = (vec![0f32; n], vec![0f32; n]);
    let mut spans = vec![];
    for (r, row) in t.rows.iter().enumerate() {
        for (k, cell) in row.cells.iter().enumerate() {
            let col = starts[r][k];
            if col >= n { continue; }
            let span = cell.colspan.min(n - col).max(1);
            let (mn, mx) = measure(&cell.items, sc, f);
            let padw = (cell.pad[1] + cell.pad[3]) * sc;
            let (mut mn, mut mx) = (mn + padw, mx + padw);
            if cell.nowrap { mn = mx; }
            if let Some(Len::Px(p)) = cell.width { mx = mn.max(p * sc + padw); }
            if span == 1 { cmin[col] = cmin[col].max(mn); cmax[col] = cmax[col].max(mx); } else { spans.push((col, span, mn, mx)); }
        }
    }
    for (col, span, mn, mx) in spans {
        for (v, need) in [(&mut cmin, mn), (&mut cmax, mx)] {
            let cur: f32 = v[col..col + span].iter().sum();
            if need > cur { let add = (need - cur) / span as f32; for c in &mut v[col..col + span] { *c += add; } }
        }
    }
    for i in 0..n { cmax[i] = cmax[i].max(cmin[i]); }
    (cmin, cmax)
}

fn table(t: &Table, left: f32, right: f32, y0: f32, sc: f32, f: &mut Fonts, out: &mut Out) -> f32 {
    let avail = right - left;
    let (cmin, cmax) = col_metrics(t, sc, f);
    let n = cmin.len();
    if n == 0 { return y0; }
    let sp = t.spacing * sc;
    let total_sp = sp * (n as f32 + 1.0);
    let (smin, smax) = (cmin.iter().sum::<f32>(), cmax.iter().sum::<f32>());
    let table_w = match t.width { Some(w) => resolve(w, avail, sc).min(avail), None => (smax + total_sp).min(avail) }.max(1.0);
    let space = (table_w - total_sp).max(0.0);
    let widths: Vec<f32> = if space >= smax {
        let extra = space - smax;
        cmax.iter().map(|m| m + if smax > 0.0 { extra * m / smax } else { extra / n as f32 }).collect()
    } else if space <= smin {
        cmin.clone()
    } else {
        let k = (space - smin) / (smax - smin);
        (0..n).map(|i| cmin[i] + (cmax[i] - cmin[i]) * k).collect()
    };
    let used = widths.iter().sum::<f32>() + total_sp;
    let tx = left + match t.align { 1 => ((avail - used) / 2.0).max(0.0), 2 => (avail - used).max(0.0), _ => 0.0 };
    let bg_pos = out.rects.len();
    let (_, starts) = grid(t);
    // pass 1: lay every cell out at the top of its row (y = 0) and measure; rowspan cells only stretch the last row they cover
    struct C<'a> { cell: &'a crate::render::Cell, tmp: Out, row: usize, cx0: f32, cw: f32, h: f32, rs: usize }
    let (mut cells, mut row_h) = (vec![], vec![0f32; t.rows.len()]);
    for (r, row) in t.rows.iter().enumerate() {
        for (k, cell) in row.cells.iter().enumerate() {
            let col = starts[r][k];
            if col >= n { continue; }
            let span = cell.colspan.min(n - col).max(1);
            let cw = widths[col..col + span].iter().sum::<f32>() + sp * (span as f32 - 1.0);
            let cx0 = tx + sp + widths[..col].iter().sum::<f32>() + sp * col as f32;
            let [pt, pr, pb, pl] = cell.pad.map(|p| p * sc);
            let mut tmp = Out::new(out.pw);
            let h = block(&cell.items, cx0 + pl, cx0 + cw - pr, pt, sc, f, &mut tmp) + pb;
            let rs = cell.rowspan.clamp(1, t.rows.len() - r);
            if rs == 1 { row_h[r] = row_h[r].max(h); }
            cells.push(C { cell, tmp, row: r, cx0, cw, h, rs });
        }
    }
    for c in cells.iter().filter(|c| c.rs > 1) {
        let last = c.row + c.rs - 1;
        let have = row_h[c.row..=last].iter().sum::<f32>() + sp * (c.rs as f32 - 1.0);
        if c.h > have { row_h[last] += c.h - have; }
    }
    let mut tops = vec![];
    let mut y = y0 + sp;
    for h in &row_h { tops.push(y); y += h + sp; }
    for (r, row) in t.rows.iter().enumerate() {
        if let Some(color) = row.bg { out.rects.push(Rect { x: tx + sp, y: tops[r], w: used - 2.0 * sp, h: row_h[r], color, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN }); }
    }
    for mut c in cells {
        let (top, full) = (tops[c.row], row_h[c.row..c.row + c.rs].iter().sum::<f32>() + sp * (c.rs as f32 - 1.0));
        if let Some(color) = c.cell.bg { out.rects.push(Rect { x: c.cx0, y: top, w: c.cw, h: full, color, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN }); }
        if t.border > 0.0 { outline(&mut out.rects, c.cx0, top, c.cw, full, (t.border * sc).round().max(1.0)); }
        let dy = top + match c.cell.valign { 1 => (full - c.h) / 2.0, 2 => full - c.h, _ => 0.0 };
        for p in &mut c.tmp.placed { p.base += dy; }
        for r in &mut c.tmp.rects { r.y += dy; }
        out.rects.extend(c.tmp.rects);
        out.placed.extend(c.tmp.placed);
    }
    if let Some(color) = t.bg { out.rects.insert(bg_pos, Rect { x: tx, y: y0, w: used, h: y - y0, color, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN }); }
    if t.border > 0.0 { outline(&mut out.rects, tx, y0, used, y - y0, (t.border * sc).round().max(1.0)); }
    y
}

fn outline(rects: &mut Vec<Rect>, x: f32, y: f32, w: f32, h: f32, b: f32) {
    let color = 0x888888;
    rects.push(Rect { x, y, w, h: b, color, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN });
    rects.push(Rect { x, y: y + h - b, w, h: b, color, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN });
    rects.push(Rect { x, y, w: b, h, color, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN });
    rects.push(Rect { x: x + w - b, y, w: b, h, color, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dom::parse, render};
    use std::collections::HashMap;

    fn fonts() -> Option<Fonts> {
        let data = ["C:\\Windows\\Fonts\\segoeui.ttf", "C:\\Windows\\Fonts\\arial.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"].iter().find_map(|p| std::fs::read(p).ok())?;
        Some(Fonts::new(fontdue::Font::from_bytes(data, fontdue::FontSettings::default()).ok()?, None))
    }

    fn lay(html: &str, width: f32) -> Option<(Vec<Placed>, Vec<Rect>, f32)> {
        let mut f = fonts()?;
        let pg = render::build(&parse(html, true), "https://a.com", HashMap::new());
        Some(layout(&pg.items, width, 1.0, &mut f))
    }

    #[test]
    fn table_columns_sit_side_by_side() {
        let Some((p, _, _)) = lay("<table cellspacing=0><tr><td>alpha</td><td>beta</td></tr><tr><td>gamma delta</td><td>x</td></tr></table>", 800.0) else { return };
        let x = |t: &str| p.iter().find(|q| q.text == t).unwrap();
        assert!(x("beta").x > x("alpha").x + 20.0);
        assert_eq!(x("beta").x, x("x").x); // same column
        assert!(x("gamma").base > x("alpha").base); // second row below
        assert_eq!(x("alpha").x, x("gamma").x);
    }

    #[test]
    fn centered_max_width_box_and_background() {
        let Some((p, r, _)) = lay("<div style='max-width:300px;margin:0 auto;background:#eee;padding:0 10px'>hello world</div>", 1000.0) else { return };
        let hello = p.iter().find(|q| q.text == "hello").unwrap();
        assert!(hello.x > 300.0 && hello.x < 400.0, "x={}", hello.x); // (968-320)/2 + 16 + padding
        let bg = r.iter().find(|q| q.color == 0xEEEEEE).unwrap();
        assert!((bg.w - 320.0).abs() < 1.0 && bg.x < hello.x);
    }

    #[test]
    fn table_width_percent_and_center_align() {
        let Some((p, _, _)) = lay("<center><table width=50%><tr><td>a</td></tr></table></center>", 1000.0) else { return };
        let a = p.iter().find(|q| q.text == "a").unwrap();
        assert!(a.x > 200.0, "x={}", a.x); // centered 484px table: starts at ~258
    }

    #[test]
    fn text_wraps_around_a_float_and_clear_ends_it() {
        let words = "word ".repeat(60);
        let Some((p, _, h)) = lay(&format!("<div style='float:right;width:200px'>box</div><p>{words}</p><div style='clear:both'>after</div>"), 600.0) else { return };
        let boxw = p.iter().find(|q| q.text == "box").unwrap();
        assert!(boxw.x > 380.0, "float on the right edge, x={}", boxw.x);
        let first = p.iter().find(|q| q.text == "word").unwrap();
        let right_edge_first_line = p.iter().filter(|q| q.text == "word" && (q.base - first.base).abs() < 1.0).map(|q| q.x + q.w).fold(0.0, f32::max);
        assert!(right_edge_first_line < boxw.x, "first line must stop before the float: {right_edge_first_line} vs {}", boxw.x);
        let last_word = p.iter().rev().find(|q| q.text == "word").unwrap();
        let after = p.iter().find(|q| q.text == "after").unwrap();
        assert!(after.base > last_word.base);
        assert!(h > after.base);
    }

    fn at<'a>(p: &'a [Placed], t: &str) -> &'a Placed { p.iter().find(|q| q.text == t).unwrap_or_else(|| panic!("no {t}")) }

    #[test]
    fn fixed_boxes_live_in_the_viewport_layer_and_z_index_orders_layers() {
        let Some((p, r, _)) = lay("<p>one</p><p>two</p><div style='position:fixed;bottom:0;left:0;width:200px;background:#abcdef'>bar</div><div style='position:absolute;top:0;left:0;z-index:5'>hi</div><div style='position:absolute;top:0;left:0;z-index:-1;background:#123456'>lo</div>", 800.0) else { return };
        let bar = at(&p, "bar");
        assert!(bar.z >= FIXED_Z && bar.base > 650.0 && bar.base <= 720.0, "bottom:0 sits at the bottom of the 720px viewport: {}", bar.base);
        assert!(r.iter().any(|q| q.color == 0xABCDEF && q.z >= FIXED_Z));
        assert!(at(&p, "hi").z > at(&p, "one").z && at(&p, "lo").z < at(&p, "one").z, "layers: {} {} {}", at(&p, "lo").z, at(&p, "one").z, at(&p, "hi").z);
        assert_eq!(layers(&p, &r).first().map(|l| l.0), Some(at(&p, "lo").z), "layers come out ascending");
        let Some((p, _, _)) = lay("<p>x</p><div style='position:absolute;top:0;left:0;background:#fff'>over</div>", 800.0) else { return };
        assert!(at(&p, "over").z > at(&p, "x").z, "positioned boxes paint above in-flow content");
    }

    #[test]
    fn column_flex_stacks_with_gap_and_aligns_across() {
        let Some((p, _, _)) = lay("<div style='display:flex;flex-direction:column;gap:20px'><div>aa</div><div>bb</div></div>", 600.0) else { return };
        assert!((at(&p, "aa").x - at(&p, "bb").x).abs() < 1.0 && at(&p, "bb").base - at(&p, "aa").base > 30.0, "stacked: {} {}", at(&p, "aa").base, at(&p, "bb").base);
        let Some((p, _, _)) = lay("<div style='display:flex;flex-direction:column;align-items:center'><div>mid</div></div>", 600.0) else { return };
        assert!(at(&p, "mid").x > 250.0, "centered across: {}", at(&p, "mid").x);
        let Some((p, _, _)) = lay("<div style='display:flex;flex-direction:column;align-items:flex-end'><div>end</div></div>", 600.0) else { return };
        assert!(at(&p, "end").x > 500.0, "end-aligned: {}", at(&p, "end").x);
    }

    #[test]
    fn grid_columns_rows_gaps_and_spans() {
        let Some((p, _, _)) = lay("<div style='display:grid;grid-template-columns:repeat(3,1fr);gap:10px'><div>a</div><div>b</div><div>c</div><div>d</div></div>", 1016.0) else { return };
        // avail 984: (984 - 20) / 3 = 321.33 per column
        let (a, b, c, d) = (at(&p, "a"), at(&p, "b"), at(&p, "c"), at(&p, "d"));
        assert!((b.x - a.x - 331.33).abs() < 2.0 && (c.x - b.x - 331.33).abs() < 2.0, "{} {} {}", a.x, b.x, c.x);
        assert!((a.base - b.base).abs() < 1.0 && d.base > a.base + 15.0 && (d.x - a.x).abs() < 1.0, "fourth item wraps to row two");
        let Some((p, _, _)) = lay("<div style='display:grid;grid-template-columns:100px 1fr'><div>l</div><div>r</div></div>", 600.0) else { return };
        assert!((at(&p, "r").x - at(&p, "l").x - 100.0).abs() < 1.0, "fixed first track");
        let Some((p, _, _)) = lay("<div style='display:grid;grid-template-columns:1fr 1fr'><div style='grid-column:span 2'>wide</div><div>n1</div><div>n2</div></div>", 600.0) else { return };
        assert!(at(&p, "n1").base > at(&p, "wide").base + 10.0 && at(&p, "n2").x > at(&p, "n1").x + 100.0);
        let Some((p, _, _)) = lay("<div style='display:grid;grid-template-columns:repeat(auto-fill,minmax(200px,1fr))'><div>1</div><div>2</div><div>3</div><div>4</div></div>", 616.0) else { return };
        // 584px fits two 200px columns; the third item starts row two
        assert!((at(&p, "1").base - at(&p, "2").base).abs() < 1.0 && at(&p, "3").base > at(&p, "1").base + 10.0, "auto-fill packs columns");
    }

    #[test]
    fn overflow_hidden_with_height_clips_and_shadow_and_gradient_emit_rects() {
        let Some((p, r, _)) = lay("<div style='height:40px;overflow:hidden;background:#abcdef'>l1<br>l2<br>l3<br>l4<br>l5<br>l6</div><p>after</p>", 600.0) else { return };
        assert!(p.iter().all(|q| q.text != "l6") && p.iter().any(|q| q.text == "l1"), "lines below the box are cut");
        let bg = r.iter().find(|q| q.color == 0xABCDEF).unwrap();
        assert!((bg.h - 40.0).abs() < 1.0 && at(&p, "after").base > bg.y + 40.0, "box is exactly 40px tall");
        let Some((_, r, _)) = lay("<div style='background:#ffffff;box-shadow:0 4px 8px rgba(0,0,0,.4)'>x</div>", 600.0) else { return };
        let shadows: Vec<_> = r.iter().filter(|q| q.color == 0 && q.a < 255).collect();
        assert!(shadows.len() >= 3 && shadows.windows(2).all(|w| w[0].w >= w[1].w), "layers shrink inward: {}", shadows.len());
        assert!(shadows[0].y > r.iter().find(|q| q.color == 0xFFFFFF).unwrap().y - 6.0);
        let Some((_, r, _)) = lay("<div style='background:linear-gradient(90deg,#ff0000,#0000ff)'>x</div><div style='background:rgba(255,0,0,.5)'>y</div>", 600.0) else { return };
        assert!(r.iter().any(|q| q.grad.is_some()) && r.iter().any(|q| q.a == 128 && q.color == 0xFF0000));
    }

    #[test]
    fn letter_spacing_vertical_align_and_opacity_change_text() {
        let Some((p, _, _)) = lay("<span>abcd</span> <span style='letter-spacing:5px'>abcd</span>", 600.0) else { return };
        let w: Vec<f32> = p.iter().map(|q| q.w).collect();
        assert!((w[1] - w[0] - 20.0).abs() < 1.0, "4 letters x 5px: {w:?}");
        let Some((p, _, _)) = lay("x<sup>up</sup>y<sub>dn</sub>", 600.0) else { return };
        let (up, dn) = (at(&p, "up"), at(&p, "dn"));
        assert!(up.base < dn.base - 3.0, "superscript sits above subscript: {} {}", up.base, dn.base);
        let Some((p, _, _)) = lay("<p style='opacity:.5;color:#000000'>faded</p>", 600.0) else { return };
        let c = at(&p, "faded").color;
        assert!(c > 0x606060 && c < 0xA0A0A0, "black at 50% over white is mid gray: {c:06x}");
    }

    fn lay_with(html: &str, width: f32, imgs: impl Fn(&crate::dom::Dom) -> HashMap<usize, crate::load::Img>) -> Option<(Vec<Placed>, Vec<Rect>, f32)> {
        let mut f = fonts()?;
        let dom = parse(html, true);
        let pg = render::build(&dom, "https://a.com", imgs(&dom));
        Some(layout(&pg.items, width, 1.0, &mut f))
    }

    fn pic(w: u32, h: u32) -> crate::load::Img { crate::load::Img { w, h, px: vec![255; (w * h * 4) as usize] } }

    #[test]
    fn percentage_heights_need_a_definite_parent_and_never_clip_the_page() {
        let Some((_, r, _)) = lay("<div style='height:200px'><div style='height:50%;background:#abcdef'>x</div></div>", 600.0) else { return };
        let bg = r.iter().find(|q| q.color == 0xABCDEF).unwrap();
        assert!((bg.h - 100.0).abs() < 1.0, "half of the 200px parent: {}", bg.h);
        let Some((_, r, _)) = lay("<div><div style='height:50%;background:#abcdef'>x</div></div>", 600.0) else { return };
        assert!(r.iter().find(|q| q.color == 0xABCDEF).unwrap().h < 40.0, "auto parent: the percentage is ignored");
        let lines = "line<br>".repeat(80);
        let Some((p, _, h)) = lay(&format!("<html style='height:100%'><body style='height:100%;overflow:hidden'>{lines}"), 600.0) else { return };
        assert!(h > 1000.0 && p.len() >= 80, "html/body are never clipped: {} placed, height {h}", p.len());
        let Some((_, r, _)) = lay("<div style='height:300px;overflow:hidden'><div style='height:50%;overflow:hidden;background:#abcdef'>a<br>b<br>c<br>d<br>e<br>f</div></div>", 600.0) else { return };
        assert!(r.iter().find(|q| q.color == 0xABCDEF).unwrap().h >= 100.0, "a percentage height is a floor here, not a clip");
    }

    #[test]
    fn percentage_width_images_fill_their_share_of_the_line() {
        let run = |css: &str| lay_with(&format!("<img src=a.png style='{css}'>"), 616.0, |d| HashMap::from([(d.find("img").unwrap(), pic(100, 40))])).map(|r| r.0);
        let (Some(a), Some(b)) = (run("width:50%"), run("width:100%")) else { return };
        let im = |p: &[Placed]| { let q = p.iter().find(|q| q.img.is_some()).unwrap(); (q.w, q.size) };
        assert!((im(&a).0 - 292.0).abs() < 1.0 && (im(&a).1 - 116.8).abs() < 1.0, "{:?}", im(&a));
        assert!((im(&b).0 - 584.0).abs() < 1.0 && (im(&b).1 - 233.6).abs() < 1.0, "{:?}", im(&b));
    }

    #[test]
    fn images_follow_vertical_align_middle_and_opacity() {
        let base = |css: &str| lay_with(&format!("x<img src=a.png width=20 height=40 style='{css}'>y"), 600.0, |d| HashMap::from([(d.find("img").unwrap(), pic(20, 40))])).map(|r| r.0);
        let (Some(a), Some(b)) = (base(""), base("vertical-align:middle")) else { return };
        let im = |p: &[Placed]| p.iter().find(|q| q.img.is_some()).unwrap().base;
        assert!(im(&b) > im(&a) + 10.0, "middle lowers a tall picture toward the text: {} vs {}", im(&b), im(&a));
        let Some(c) = base("opacity:.5") else { return };
        assert_eq!(c.iter().find(|q| q.img.is_some()).unwrap().color, 128);
        assert_eq!(a.iter().find(|q| q.img.is_some()).unwrap().color, 255);
    }

    #[test]
    fn background_images_are_sized_positioned_and_repeated() {
        let html = |css: &str| format!("<div id=b style='height:100px;{css}'>x</div>");
        let run = |css: &str| lay_with(&html(css), 616.0, |d| HashMap::from([(usize::MAX - d.find("div").unwrap(), pic(40, 20))])).map(|r| r.1);
        let (Some(r), Some(c), Some(v)) = (run("background:url(a.png) no-repeat right center / 20px 20px"), run("background:url(a.png) center / cover no-repeat"), run("background-image:url(a.png)")) else { return };
        let p = r.iter().find_map(|q| q.pic).expect("image rect");
        assert!((p.w, p.h) == (20.0, 20.0) && !p.rx && !p.ry, "{} {}", p.w, p.h);
        assert!((p.ox - (584.0 - 20.0)).abs() < 1.0 && (p.oy - 40.0).abs() < 1.0, "right center: {} {}", p.ox, p.oy);
        let p = c.iter().find_map(|q| q.pic).unwrap();
        assert!(p.w >= 584.0 - 0.5 && p.h >= 100.0 - 0.5, "cover fills the box: {} {}", p.w, p.h);
        let p = v.iter().find_map(|q| q.pic).unwrap();
        assert!((p.w, p.h, p.rx, p.ry) == (40.0, 20.0, true, true), "natural size, tiled both ways");
    }

    #[test]
    fn fixed_and_sticky_boxes_get_a_pin_offset() {
        let Some((p, r, _)) = lay("<p>a</p><p>b</p><div style='position:sticky;top:10px;background:#abcdef'>stuck</div><div style='position:fixed;top:0;left:0'>bar</div>", 600.0) else { return };
        let stuck = at(&p, "stuck");
        assert!(stuck.st.is_finite() && (stuck.st - (stuck.base - stuck.size - 10.0)).abs() < 25.0, "pins when its top reaches 10px below the viewport top: st {} base {}", stuck.st, stuck.base);
        assert!(r.iter().find(|q| q.color == 0xABCDEF).unwrap().st.is_finite());
        assert_eq!(at(&p, "bar").st, 0.0, "fixed = pinned from the first pixel of scrolling");
        assert!(at(&p, "a").st.is_nan());
        assert!((stuck.base - at(&p, "b").base) > 10.0, "sticky keeps its normal place until the page scrolls");
    }

    #[test]
    fn nested_stacking_contexts_cannot_escape_their_parent() {
        let Some((p, _, _)) = lay("<div style='position:absolute;top:0;left:0;z-index:5'>outer<div style='position:absolute;top:0;left:0;z-index:100'>deep</div></div><div style='position:absolute;top:0;left:0;z-index:6'>sibling</div><p>flow</p>", 800.0) else { return };
        let (outer, deep, sib, flow) = (at(&p, "outer").z, at(&p, "deep").z, at(&p, "sibling").z, at(&p, "flow").z);
        assert!(deep > outer, "a child paints above its own context's content");
        assert!(deep < sib, "but a z-index:100 child of a z-index:5 context stays below a z-index:6 sibling: {deep} vs {sib}");
        assert!(outer > flow && sib > outer);
        // a relative box with a z-index is a context too
        let Some((p, _, _)) = lay("<div style='position:relative;z-index:2'>rel<div style='position:absolute;top:0;z-index:50'>kid</div></div><div style='position:absolute;z-index:3'>top</div>", 800.0) else { return };
        assert!(at(&p, "kid").z < at(&p, "top").z && at(&p, "kid").z > at(&p, "rel").z);
        let Some((p, _, _)) = lay("<div style='position:fixed;top:0;z-index:1'>fix</div><div style='position:absolute;z-index:9999'>abs</div>", 800.0) else { return };
        assert!(at(&p, "fix").z > at(&p, "abs").z, "fixed boxes paint on top of everything");
    }

    #[test]
    fn sticky_boxes_are_held_back_by_their_container() {
        let Some((p, _, _)) = lay("<div style='height:300px'><div style='position:sticky;top:0'>s</div></div><p>after</p>", 600.0) else { return };
        let s = at(&p, "s");
        assert!(s.lim.is_finite() && s.lim > 250.0 && s.lim < 300.0, "may travel about the container's height: {}", s.lim);
        let Some((p, _, _)) = lay("<div style='position:sticky;top:0'>solo</div><p>x</p><p>y</p>", 600.0) else { return };
        assert!(at(&p, "solo").lim > 20.0, "inside the page the container is the page");
        assert!(at(&p, "x").lim.is_nan());
    }

    #[test]
    fn plain_boxes_leave_an_invisible_geometry_marker() {
        let Some((_, r, _)) = lay("<div style='width:200px;padding:6px'>text</div>", 600.0) else { return };
        let m = r.iter().find(|q| q.node != 0 && q.a == 0).expect("marker for a box without background");
        assert!((m.w - 212.0).abs() < 1.5 && m.h > 20.0 && m.scroll.is_nan(), "{} x {}", m.w, m.h);
        let Some((p, r, _)) = lay("<div style='width:200px;padding:6px;background:#abcdef'>text</div>", 600.0) else { return };
        assert!(r.iter().all(|q| q.a != 0) && !p.is_empty(), "a box that has a background needs no marker");
        assert_eq!(crate::browse::hit_node(&[], &r, 20.0, 25.0) != 0, true, "hit-testing finds the box in its empty padding");
    }

    #[test]
    fn overflow_auto_with_a_height_makes_a_scroll_container() {
        let Some((p, r, _)) = lay("<div style='height:60px;overflow:auto;background:#abcdef'>l1<br>l2<br>l3<br>l4<br>l5<br>l6<br>l7<br>l8</div><p>after</p>", 600.0) else { return };
        let g = groups(&r);
        assert_eq!(g.len(), 1);
        let b = g.values().next().unwrap();
        assert!((b.h - 60.0).abs() < 1.0 && b.content_h > 150.0, "box {} content {}", b.h, b.content_h);
        assert!(p.iter().any(|q| q.text == "l8" && q.grp != 0), "content is kept (not cut) and tagged with the container");
        assert!(at(&p, "after").grp == 0 && at(&p, "after").base > b.y + 60.0, "the page continues below the 60px box");
        let bg = r.iter().find(|q| q.color == 0xABCDEF).unwrap();
        assert!((bg.h - 60.0).abs() < 1.0 && bg.grp == 0, "the background stays still while the content scrolls");
        // overflow: hidden keeps cutting instead
        let Some((p, r, _)) = lay("<div style='height:60px;overflow:hidden'>l1<br>l2<br>l3<br>l4<br>l5<br>l6<br>l7<br>l8</div>", 600.0) else { return };
        assert!(groups(&r).is_empty() && p.iter().all(|q| q.text != "l8"));
        // a container without a fixed height does not scroll
        let Some((_, r, _)) = lay("<div style='overflow:auto'>l1<br>l2<br>l3</div>", 600.0) else { return };
        assert!(groups(&r).is_empty());
    }

    #[test]
    fn counters_number_ordered_lists_and_generated_content() {
        let Some((p, _, _)) = lay("<ol><li>a</li><li>b</li></ol><ol start=5><li>c</li></ol>", 600.0) else { return };
        let marks: Vec<&str> = p.iter().filter(|q| q.text.ends_with('.')).map(|q| q.text.as_str()).collect();
        assert_eq!(marks, vec!["1.", "2.", "5."]);
        let Some((p, _, _)) = lay("<style>body{counter-reset:sec} h2{counter-increment:sec} h2::before{content:'Part ' counter(sec) ': '}</style><h2>one</h2><h2>two</h2>", 600.0) else { return };
        assert!(p.iter().any(|q| q.text == "1:") && p.iter().any(|q| q.text == "2:"), "{:?}", p.iter().map(|q| q.text.clone()).collect::<Vec<_>>());
    }

    #[test]
    fn line_height_spaces_lines() {
        let Some((p, _, _)) = lay("<p style='line-height:3'>aaa<br>bbb</p><p>ccc<br>ddd</p>", 600.0) else { return };
        let (a, b, c, d) = (at(&p, "aaa").base, at(&p, "bbb").base, at(&p, "ccc").base, at(&p, "ddd").base);
        assert!(((b - a) - 48.0).abs() < 1.0, "3x16px lines: {}", b - a);
        assert!(((d - c) - 21.6).abs() < 1.0, "default 1.35: {}", d - c);
    }

    #[test]
    fn inline_blocks_sit_side_by_side_and_wrap() {
        let Some((p, r, _)) = lay("<span style='display:inline-block;width:100px;background:#abcdef'>one</span><span style='display:inline-block;width:100px'>two</span><span style='display:inline-block;width:300px'>three</span>", 400.0) else { return };
        let (one, two, three) = (at(&p, "one"), at(&p, "two"), at(&p, "three"));
        assert!(two.x > one.x + 90.0 && (two.base - one.base).abs() < 1.0, "same line: {} {}", one.x, two.x);
        assert!(three.base > two.base + 5.0, "third wraps to the next line");
        assert!(r.iter().any(|q| q.color == 0xABCDEF && (q.w - 100.0).abs() < 1.0));
    }

    #[test]
    fn flex_row_grow_justify_and_gap() {
        let Some((p, _, _)) = lay("<div style='display:flex;gap:10px'><div style='flex:1'>a</div><div style='flex:3'>b</div></div>", 1016.0) else { return };
        // avail 984: flex:1 and flex:3 split (984-10) as 1:3 -> second starts at 16 + 243.5 + 10
        let (a, b) = (at(&p, "a"), at(&p, "b"));
        assert!((b.x - a.x - 253.5).abs() < 2.0, "{}", b.x - a.x);
        let Some((p, _, _)) = lay("<div style='display:flex;justify-content:space-between'><span>l</span><span>r</span></div>", 600.0) else { return };
        assert!(at(&p, "r").x > 500.0 && at(&p, "l").x < 40.0);
        let Some((p, _, _)) = lay("<div style='display:flex'><span>l</span><span style='margin-left:auto'>r</span></div>", 600.0) else { return };
        assert!(at(&p, "r").x > 500.0, "auto margin pushes right: {}", at(&p, "r").x);
    }

    #[test]
    fn flex_wrap_and_align_center() {
        let Some((p, _, _)) = lay("<div style='display:flex;flex-wrap:wrap'><div style='width:300px'>a</div><div style='width:300px'>b</div><div style='width:300px'>c</div></div>", 700.0) else { return };
        assert!(at(&p, "c").base > at(&p, "a").base + 5.0 && (at(&p, "b").base - at(&p, "a").base).abs() < 1.0);
        let Some((p, _, _)) = lay("<div style='display:flex;align-items:center'><div>x<br>y<br>z</div><div>m</div></div>", 600.0) else { return };
        assert!((at(&p, "m").base - at(&p, "y").base).abs() < 1.0, "m centered against 3-line sibling");
    }

    #[test]
    fn absolute_and_relative_positioning() {
        let Some((p, _, _)) = lay("<div style='position:relative;height:200px'>top<span style='position:absolute;right:10px;top:50px;width:60px'>abs</span></div>", 800.0) else { return };
        let (t, a) = (at(&p, "top"), at(&p, "abs"));
        assert!((a.x - (800.0 - 16.0 - 10.0 - 60.0)).abs() < 2.0, "right edge of containing block: {}", a.x);
        assert!(a.base > t.base + 40.0, "top offset applies: {} vs {}", a.base, t.base);
        let Some((p, _, h)) = lay("<p>before</p><div style='position:relative;left:30px;top:5px'>moved</div><p>after</p>", 800.0) else { return };
        let (b, m, af) = (at(&p, "before"), at(&p, "moved"), at(&p, "after"));
        assert!((m.x - b.x - 30.0).abs() < 1.0 && m.base > b.base + 20.0);
        assert!(af.base > m.base && h > af.base);
        let Some((p, _, _)) = lay("<p>one</p><div style='position:absolute;left:100px;top:300px'>free</div><p>two</p>", 800.0) else { return };
        assert!((at(&p, "free").x - 100.0).abs() < 1.0 && (at(&p, "free").base - 300.0).abs() < 30.0);
        assert!(at(&p, "two").base < 120.0, "absolute takes no flow space");
    }

    #[test]
    fn min_height_and_border_box() {
        let Some((p, r, _)) = lay("<div style='min-height:100px;background:#abcdef'>x</div><p>after</p>", 600.0) else { return };
        let bg = r.iter().find(|q| q.color == 0xABCDEF).unwrap();
        assert!(bg.h >= 100.0 && at(&p, "after").base > bg.y + 100.0, "h={}", bg.h);
        let Some((_, r, _)) = lay("<div style='box-sizing:border-box;width:300px;padding:0 20px;background:#abcdef'>x</div>", 600.0) else { return };
        assert!((r.iter().find(|q| q.color == 0xABCDEF).unwrap().w - 300.0).abs() < 1.0);
        let Some((_, r, _)) = lay("<div style='width:300px;padding:0 20px;background:#abcdef'>x</div>", 600.0) else { return };
        assert!((r.iter().find(|q| q.color == 0xABCDEF).unwrap().w - 340.0).abs() < 1.0);
    }

    #[test]
    fn rowspan_skips_covered_columns_and_stretches_rows() {
        let Some((p, _, _)) = lay("<table cellspacing=0><tr><td rowspan=2>tall<br>tall2<br>tall3</td><td>a</td></tr><tr><td>b</td><td>c</td></tr><tr><td>d</td><td>e</td></tr></table>", 800.0) else { return };
        let (a, b, c, d) = (at(&p, "a"), at(&p, "b"), at(&p, "c"), at(&p, "d"));
        assert!(b.x > a.x - 1.0 && (b.x - a.x).abs() < 1.0, "b sits in a's column, right of the tall cell: {} {}", a.x, b.x);
        assert!(c.x > b.x + 5.0, "c is the extra third column");
        assert!(d.base > at(&p, "tall3").base - 1.0, "row 3 starts below the tall cell: {} vs {}", d.base, at(&p, "tall3").base);
        assert!(at(&p, "tall").x < a.x);
    }

    #[test]
    fn inline_block_shrinks_to_fit_with_padding() {
        let Some((p, _, _)) = lay("<span style='display:inline-block;padding:0 14px;background:#abcdef'>pill button</span>", 600.0) else { return };
        assert!((at(&p, "pill").base - at(&p, "button").base).abs() < 1.0, "one line");
    }

    #[test]
    fn nowrap_keeps_a_run_together_and_text_indent_moves_the_first_line() {
        let Some((p, _, _)) = lay("<div style='width:100px;white-space:nowrap'>alpha beta gamma</div>", 600.0) else { return };
        assert_eq!(p.len(), 1, "one unbreakable run");
        let Some((p, _, _)) = lay("<p style='text-indent:40px'>first line text that is long enough to wrap onto a second line in this narrow box of text</p>", 220.0) else { return };
        let first = p.iter().find(|q| q.text == "first").unwrap();
        let second_line = p.iter().find(|q| q.base > first.base + 5.0).unwrap();
        assert!((first.x - 56.0).abs() < 1.0 && (second_line.x - 16.0).abs() < 1.0, "{} {}", first.x, second_line.x);
        let Some((p, _, _)) = lay("<h1 style='text-indent:-9999px'>hidden</h1><p>shown</p>", 600.0) else { return };
        assert!(p.iter().all(|q| q.text != "hidden"));
    }

    #[test]
    fn rounded_boxes_emit_rounded_rects_and_rings() {
        let Some((_, r, _)) = lay("<div style='border-radius:8px;background:#abcdef;border:2px solid #ff0000'>x</div>", 600.0) else { return };
        assert!(r.iter().any(|q| q.color == 0xABCDEF && q.r == 8.0 && q.ring == 0.0));
        assert!(r.iter().any(|q| q.color == 0xFF0000 && q.r == 8.0 && q.ring == 2.0));
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn borders_and_rules_produce_rects() {
        let Some((_, r, _)) = lay("<div style='border:2px solid #ff0000;padding:4px'>x</div><hr>", 600.0) else { return };
        assert!(r.iter().filter(|q| q.color == 0xFF0000).count() == 4);
        assert!(r.iter().any(|q| q.color == 0x999999 && q.w > 500.0));
    }

    #[test]
    fn rtl_lines_run_right_to_left_and_align_right() {
        let Some((p, _, _)) = lay("<p>\u{05e9}\u{05dc}\u{05d5}\u{05dd} \u{05e2}\u{05d5}\u{05dc}\u{05dd}</p>", 600.0) else { return };
        assert_eq!(p.len(), 2);
        assert!(p[0].x > p[1].x, "first word is on the right: {} {}", p[0].x, p[1].x);
        assert!(p[0].x + p[0].w > 560.0, "right aligned");
        let Some((p, _, _)) = lay("<p>one two</p>", 600.0) else { return };
        assert!(p[0].x < p[1].x && p[0].x < 20.0);
    }

    #[test]
    fn uax9_numbers_inside_rtl_keep_their_order_and_brackets_mirror() {
        // RTL paragraph: Hebrew, a number, Hebrew; logical order 1 2 3 shows as 3 2 1 from the left, the number stays one word
        let Some((p, _, _)) = lay("<p>\u{05d0}\u{05d1} 123 \u{05d2}\u{05d3}</p>", 600.0) else { return };
        let (a, n, g) = (&p[0], &p[1], &p[2]);
        assert!(a.x > n.x && n.x > g.x, "visual order right to left: {} {} {}", a.x, n.x, g.x);
        // a run of two numbers separated by a space keeps left-to-right order inside the RTL line (level 2)
        let Some((p, _, _)) = lay("<p>\u{05d0}\u{05d1} 12 34 \u{05d2}\u{05d3}</p>", 600.0) else { return };
        assert!(p[1].x < p[2].x, "12 comes before 34 from the left: {} {}", p[1].x, p[2].x);
        assert!(p[0].x > p[2].x && p[3].x < p[1].x);
        // a lone bracket in an RTL line is mirrored
        let Some((p, _, _)) = lay("<p>\u{05d0}\u{05d1} (</p>", 600.0) else { return };
        assert_eq!(p[1].text, ")");
        // an all-Latin line is untouched
        let Some((p, _, _)) = lay("<p>(a) b</p>", 600.0) else { return };
        assert_eq!(p[0].text, "(a)");
    }

    #[test]
    fn soft_hyphen_shows_a_hyphen_only_where_the_line_breaks() {
        let Some((p, _, _)) = lay("<p style='width:50px'>co\u{ad}operate</p>", 600.0) else { return };
        assert!(p.iter().any(|q| q.text == "co-") && p.iter().any(|q| q.text == "operate"), "{:?}", p.iter().map(|q| q.text.clone()).collect::<Vec<_>>());
        assert!(p.iter().find(|q| q.text == "operate").unwrap().base > p.iter().find(|q| q.text == "co-").unwrap().base + 5.0, "broken onto two lines");
        let Some((p, _, _)) = lay("<p>co\u{ad}operate</p>", 600.0) else { return };
        assert!(p.iter().all(|q| !q.text.contains('-') && !q.text.contains('\u{ad}')), "{:?}", p.iter().map(|q| q.text.clone()).collect::<Vec<_>>());
        assert!((p[0].base - p[1].base).abs() < 1.0 && (p[1].x - (p[0].x + p[0].w)).abs() < 1.0, "pieces sit together when it fits");
    }

    #[test]
    fn long_urls_and_hyphenated_words_wrap_at_uax14_opportunities() {
        let Some((p, _, h)) = lay("<p style='width:120px'>see https://example.com/some/long/path/name today</p>", 600.0) else { return };
        assert!(p.iter().any(|q| q.text == "path/") && h > 100.0, "long URL is broken at slashes instead of overflowing: {:?}", p.iter().map(|q| q.text.clone()).collect::<Vec<_>>());
        assert!(p.iter().all(|q| q.x + q.w < 16.0 + 140.0), "nothing sticks out of the 120px box");
    }

    #[test]
    fn mixed_direction_lines_keep_each_run_readable() {
        // LTR paragraph with a Hebrew run: label stays on the left, the two Hebrew words are swapped
        let Some((p, _, _)) = lay("<p>Label \u{05e9}\u{05dc}\u{05d5}\u{05dd} \u{05e2}\u{05d5}\u{05dc}\u{05dd}</p>", 600.0) else { return };
        assert!(p[0].x < 40.0 && p[1].x > p[2].x, "{:?}", p.iter().map(|q| q.x).collect::<Vec<_>>());
        // RTL paragraph with a Latin run: the Latin words keep left-to-right order
        let Some((p, _, _)) = lay("<p>\u{05e9}\u{05dc}\u{05d5}\u{05dd} one two</p>", 600.0) else { return };
        let (heb, one, two) = (&p[0], &p[1], &p[2]);
        assert!(heb.x > one.x && one.x < two.x, "{} {} {}", heb.x, one.x, two.x);
    }
}
