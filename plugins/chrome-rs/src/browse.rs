// Window-independent page interaction: find-in-page and text selection over laid-out words (all coordinates are layout px).
// shortcut: selection follows layout order of words (not columns/floats), RTL words map x to characters mirrored but
// selection across mixed-direction runs is approximate.
use crate::{gfx::{is_rtl, Fonts}, layout::Placed};

/// Highlight pieces of one match: (index into `placed`, x offset inside the word, width).
pub struct Match { pub parts: Vec<(usize, f32, f32)> }

fn lower(c: char) -> char { c.to_lowercase().next().unwrap_or(c) }

fn is_text(p: &Placed) -> bool { !p.text.is_empty() && p.img.is_none() && p.field.is_none() }

/// Width of characters `from..to` of `p`'s text.
fn span_w(p: &Placed, from: usize, to: usize, f: &mut Fonts) -> f32 {
    if from >= to { return 0.0; }
    f.width_f(&p.text.chars().skip(from).take(to - from).collect::<String>(), p.size, p.face)
}

/// All case-insensitive occurrences of `q` in the page text (words are joined by single spaces, so phrases match across words).
pub fn find_matches(placed: &[Placed], q: &str, f: &mut Fonts) -> Vec<Match> {
    let qc: Vec<char> = q.split_whitespace().collect::<Vec<_>>().join(" ").chars().map(lower).collect();
    if qc.is_empty() { return vec![]; }
    let (mut hay, mut src): (Vec<char>, Vec<Option<(usize, usize)>>) = (vec![], vec![]);
    for (i, p) in placed.iter().enumerate().filter(|(_, p)| is_text(p)) {
        if !hay.is_empty() { hay.push(' '); src.push(None); }
        for (k, c) in p.text.chars().enumerate() { hay.push(lower(c)); src.push(Some((i, k))); }
    }
    let mut out = vec![];
    let mut s = 0;
    while s + qc.len() <= hay.len() && out.len() < 1000 {
        if hay[s..s + qc.len()] == qc[..] {
            let mut parts: Vec<(usize, usize, usize)> = vec![]; // (placed, first char, last char + 1)
            for &(i, k) in src[s..s + qc.len()].iter().flatten() {
                match parts.last_mut() { Some(l) if l.0 == i => l.2 = k + 1, _ => parts.push((i, k, k + 1)) }
            }
            out.push(Match { parts: parts.into_iter().map(|(i, a, b)| (i, span_w(&placed[i], 0, a, f), span_w(&placed[i], a, b, f))).collect() });
            s += qc.len();
        } else { s += 1; }
    }
    out
}

/// A caret position inside the page text: word index and character offset.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SelPoint { pub idx: usize, pub ch: usize }

/// Nearest text position to the layout point (x, y); None when the page has no text.
pub fn hit_point(placed: &[Placed], x: f32, y: f32, f: &mut Fonts) -> Option<SelPoint> {
    let dist = |p: &Placed| {
        let (top, bot) = (p.base - p.size, p.base + p.size * 0.3);
        let dy = if y < top { top - y } else if y > bot { y - bot } else { 0.0 };
        let dx = if x < p.x { p.x - x } else if x > p.x + p.w { x - p.x - p.w } else { 0.0 };
        dy * 4.0 + dx
    };
    let (idx, p) = placed.iter().enumerate().filter(|(_, p)| is_text(p)).min_by(|a, b| dist(a.1).total_cmp(&dist(b.1)))?;
    let rel = if is_rtl(&p.text) { p.w - (x - p.x) } else { x - p.x };
    let (mut acc, mut ch) = (0.0, 0);
    for (k, c) in p.text.chars().enumerate() {
        let cw = f.width_f(&c.to_string(), p.size, p.face);
        if rel < acc + cw / 2.0 { return Some(SelPoint { idx, ch: k }); }
        acc += cw;
        ch = k + 1;
    }
    Some(SelPoint { idx, ch })
}

fn ordered(a: SelPoint, b: SelPoint) -> (SelPoint, SelPoint) { if (a.idx, a.ch) <= (b.idx, b.ch) { (a, b) } else { (b, a) } }

/// (word index, first char, last char + 1) of every selected piece in reading order.
fn pieces(placed: &[Placed], a: SelPoint, b: SelPoint) -> Vec<(usize, usize, usize)> {
    let (a, b) = ordered(a, b);
    (a.idx..=b.idx.min(placed.len().saturating_sub(1))).filter(|&i| is_text(&placed[i])).filter_map(|i| {
        let len = placed[i].text.chars().count();
        let (s, e) = (if i == a.idx { a.ch } else { 0 }, if i == b.idx { b.ch.min(len) } else { len });
        (s < e).then_some((i, s, e))
    }).collect()
}

/// Plain text of the selection: words separated by spaces, new lines where the baseline changes.
pub fn selected_text(placed: &[Placed], a: SelPoint, b: SelPoint) -> String {
    let mut out = String::new();
    let mut prev: Option<&Placed> = None;
    for (i, s, e) in pieces(placed, a, b) {
        let p = &placed[i];
        if let Some(q) = prev { out.push(if p.base - q.base > q.size.min(p.size) * 0.6 { '\n' } else { ' ' }); }
        out.extend(p.text.chars().skip(s).take(e - s));
        prev = Some(p);
    }
    out
}

/// Highlight rectangles (x, top, w, h) for the selection.
pub fn selection_rects(placed: &[Placed], a: SelPoint, b: SelPoint, f: &mut Fonts) -> Vec<(f32, f32, f32, f32)> {
    pieces(placed, a, b).into_iter().map(|(i, s, e)| {
        let p = &placed[i];
        let len = p.text.chars().count();
        let (off, w) = if s == 0 && e == len { (0.0, p.w) } else { (span_w(p, 0, s, f), span_w(p, s, e, f)) };
        (p.x + off, p.base - p.size, w, p.size * 1.25)
    }).collect()
}

/// Whole-word selection around a point (double click).
pub fn word_at(placed: &[Placed], pt: SelPoint) -> (SelPoint, SelPoint) {
    let len = placed.get(pt.idx).map_or(0, |p| p.text.chars().count());
    (SelPoint { idx: pt.idx, ch: 0 }, SelPoint { idx: pt.idx, ch: len })
}

/// First and last text position of the page (select all).
pub fn all(placed: &[Placed]) -> Option<(SelPoint, SelPoint)> {
    let first = placed.iter().position(is_text)?;
    let last = placed.iter().rposition(is_text)?;
    Some((SelPoint { idx: first, ch: 0 }, SelPoint { idx: last, ch: placed[last].text.chars().count() }))
}

/// Element under a layout point: the word/picture there, else the topmost box rectangle (0 = nothing).
pub fn hit_node(placed: &[Placed], rects: &[crate::layout::Rect], x: f32, y: f32) -> usize {
    placed.iter().rev().find(|p| p.node != 0 && x >= p.x && x <= p.x + p.w && y >= p.base - p.size && y <= p.base + p.size * 0.3).map(|p| p.node)
        .or_else(|| rects.iter().rev().find(|r| r.node != 0 && x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h).map(|r| r.node))
        .unwrap_or(0)
}

/// Bounding boxes `(node, x, y, w, h)` in CSS px (layout px / `scale`) of every node that has painted content, including
/// ancestors (the union of what they contain). `parents[i]` is the DOM parent of node `i`.
pub fn node_boxes(placed: &[Placed], rects: &[crate::layout::Rect], parents: &[usize], scale: f32) -> Vec<(usize, f32, f32, f32, f32)> {
    let mut map: std::collections::HashMap<usize, (f32, f32, f32, f32)> = std::collections::HashMap::new(); // x0, y0, x1, y1
    let mut add = |node: usize, b: (f32, f32, f32, f32)| {
        let mut n = node;
        while n != 0 {
            let e = map.entry(n).or_insert(b);
            *e = (e.0.min(b.0), e.1.min(b.1), e.2.max(b.2), e.3.max(b.3));
            n = parents.get(n).copied().unwrap_or(0);
        }
    };
    for p in placed.iter().filter(|p| p.node != 0) { add(p.node, (p.x, p.base - p.size, p.x + p.w, p.base + p.size * 0.25)); }
    for r in rects.iter().filter(|r| r.node != 0 && r.ring == 0.0) { add(r.node, (r.x, r.y, r.x + r.w, r.y + r.h)); }
    map.into_iter().map(|(n, b)| (n, b.0 / scale, b.1 / scale, (b.2 - b.0) / scale, (b.3 - b.1) / scale)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dom::parse, layout::layout, render};
    use std::collections::HashMap;

    fn lay(html: &str) -> Option<(Vec<Placed>, Fonts)> {
        let mut f = Fonts::system()?;
        let pg = render::build(&parse(html, true), "https://a.com", HashMap::new());
        let (p, _, _) = layout(&pg.items, 800.0, 1.0, &mut f);
        Some((p, f))
    }

    #[test]
    fn find_is_case_insensitive_and_matches_phrases_across_words() {
        let Some((p, mut f)) = lay("<p>Alpha beta<br>gamma BETA delta</p>") else { return };
        let m = find_matches(&p, "beta", &mut f);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].parts.len(), 1);
        let ph = find_matches(&p, "alpha  beta", &mut f);
        assert_eq!((ph.len(), ph[0].parts.len()), (1, 2), "a phrase spans two words");
        let part = find_matches(&p, "amm", &mut f);
        assert!(part[0].parts[0].1 > 0.0 && part[0].parts[0].2 > 0.0, "inside-word match has an offset and a width");
        assert!(find_matches(&p, "nothing", &mut f).is_empty() && find_matches(&p, "  ", &mut f).is_empty());
    }

    #[test]
    fn selection_points_text_and_rects() {
        let Some((p, mut f)) = lay("<p>alpha beta<br>gamma delta</p>") else { return };
        let beta = p.iter().find(|q| q.text == "beta").unwrap();
        let a = hit_point(&p, beta.x + 1.0, beta.base - 5.0, &mut f).unwrap();
        assert_eq!((p[a.idx].text.as_str(), a.ch), ("beta", 0));
        let gamma = p.iter().find(|q| q.text == "gamma").unwrap();
        let b = hit_point(&p, gamma.x + gamma.w - 1.0, gamma.base - 5.0, &mut f).unwrap();
        assert_eq!((p[b.idx].text.as_str(), b.ch), ("gamma", 5));
        assert_eq!(selected_text(&p, a, b), "beta\ngamma");
        assert_eq!(selected_text(&p, b, a), "beta\ngamma", "order does not matter");
        let rects = selection_rects(&p, a, b, &mut f);
        assert_eq!(rects.len(), 2);
        let (s, e) = all(&p).unwrap();
        assert_eq!(selected_text(&p, s, e), "alpha beta\ngamma delta");
        let (w0, w1) = word_at(&p, a);
        assert_eq!(selected_text(&p, w0, w1), "beta");
        let far = hit_point(&p, 5000.0, 5000.0, &mut f).unwrap();
        assert_eq!(p[far.idx].text, "delta", "points outside the text snap to the nearest word");
    }

    #[test]
    fn hit_nodes_and_boxes_follow_the_dom() {
        let mut f = match Fonts::system() { Some(f) => f, None => return };
        let d = parse("<div id=a style='background:#eee;padding:0 8px'><span id=b>hello</span> world</div>", true);
        let pg = render::build(&d, "https://a.com", HashMap::new());
        let (placed, rects, _) = layout(&pg.items, 800.0, 1.0, &mut f);
        let id_of = |name: &str| d.descendants().into_iter().find(|&i| d.attr(i, "id") == Some(name)).unwrap();
        let hello = placed.iter().find(|p| p.text == "hello").unwrap();
        let text_node = hit_node(&placed, &rects, hello.x + 2.0, hello.base - 4.0);
        assert_eq!(d.nodes[text_node].parent, id_of("b"), "a word belongs to its text node, whose parent is the span");
        assert_eq!(hit_node(&placed, &rects, 790.0, 5000.0), 0);
        let boxes = node_boxes(&placed, &rects, &pg.parents, 1.0);
        let b = |n: usize| boxes.iter().find(|x| x.0 == n).copied().unwrap();
        let (span, div) = (b(id_of("b")), b(id_of("a")));
        assert!(span.4 > 5.0 && span.3 > 20.0 && span.2 >= div.2 - 0.5, "span {span:?} div {div:?}");
        assert!(div.1 + div.3 >= span.1 + span.3 + 20.0, "the div contains the span plus ' world': {div:?} {span:?}");
    }
}
