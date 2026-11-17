// SVG support: standalone .svg files and inline <svg> elements are rasterized once (resvg) into an RGBA Img.
// shortcut: no <text> (fonts are not loaded into resvg), no page-CSS styling of inline svg; upgrade when icon fonts/labels matter.
use crate::{dom::Dom, load::Img};
use resvg::{tiny_skia, usvg};

const MAX_W: f32 = 800.0;

/// Rasterizes SVG markup at its natural size (scaled down to MAX_W wide). Straight (non-premultiplied) RGBA.
pub fn render(svg: &str) -> Option<Img> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let k = (MAX_W / size.width()).min(1.0);
    let (w, h) = (((size.width() * k).ceil() as u32).clamp(1, 4000), ((size.height() * k).ceil() as u32).clamp(1, 4000));
    let mut pm = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(k, k), &mut pm.as_mut());
    let mut px = pm.take();
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a > 0 && a < 255 { for c in &mut p[..3] { *c = ((*c as u32 * 255 / a).min(255)) as u8; } }
    }
    Some(Img { w, h, px })
}

const CAMEL_ATTRS: &[&str] = &[
    "viewBox", "preserveAspectRatio", "gradientUnits", "gradientTransform", "patternUnits", "patternContentUnits", "patternTransform",
    "clipPathUnits", "maskUnits", "maskContentUnits", "markerWidth", "markerHeight", "markerUnits", "refX", "refY", "stdDeviation",
    "spreadMethod", "startOffset", "textLength", "baseFrequency", "numOctaves", "filterUnits", "primitiveUnits", "pathLength",
];
const CAMEL_TAGS: &[&str] = &[
    "linearGradient", "radialGradient", "clipPath", "textPath", "foreignObject", "feGaussianBlur", "feOffset", "feMerge", "feMergeNode",
    "feColorMatrix", "feBlend", "feFlood", "feComposite",
];

/// The HTML parser lowercases names; SVG is case-sensitive, so restore the common camelCase ones.
fn restore<'a>(lower: &'a str, table: &'a [&'a str]) -> &'a str {
    table.iter().find(|c| c.eq_ignore_ascii_case(lower)).copied().unwrap_or(lower)
}

fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;") }

/// Serializes the element subtree at `id` as standalone SVG markup. `<use href="#x">` that points outside the subtree (an icon sprite
/// elsewhere in the page) gets its target copied into a trailing `<defs>`, since the rasterizer only sees this one element.
pub fn to_markup(dom: &Dom, id: usize) -> String {
    let mut out = String::new();
    write(dom, id, true, &mut out);
    let (mut have, mut pending): (Vec<String>, Vec<usize>) = (vec![], vec![id]);
    let mut defs = String::new();
    let ids_in = |root: usize| { let mut v = vec![]; walk(dom, root, &mut |i| if let Some(x) = dom.attr(i, "id") { v.push(x.to_string()); }); v };
    have.extend(ids_in(id));
    for _round in 0..3 {
        let mut wanted: Vec<String> = vec![];
        for r in pending.drain(..) {
            walk(dom, r, &mut |i| if dom.nodes[i].tag == "use" {
                if let Some(h) = dom.attr(i, "href").or_else(|| dom.attr(i, "xlink:href")).and_then(|h| h.strip_prefix('#')) { if !have.iter().any(|x| x == h) && !wanted.iter().any(|x| x == h) { wanted.push(h.to_string()); } }
            });
        }
        if wanted.is_empty() || have.len() > 60 { break; }
        for w in wanted {
            let Some(t) = (1..dom.nodes.len()).find(|&i| dom.attr(i, "id") == Some(w.as_str()) && !dom.nodes[i].tag.is_empty()) else { continue };
            write(dom, t, false, &mut defs);
            have.push(w);
            pending.push(t);
        }
    }
    if !defs.is_empty() && out.ends_with("</svg>") { out.truncate(out.len() - 6); out.push_str(&format!("<defs>{defs}</defs></svg>")); }
    out
}

fn walk(dom: &Dom, id: usize, f: &mut dyn FnMut(usize)) {
    f(id);
    for &k in &dom.nodes[id].kids { walk(dom, k, f); }
}

fn write(dom: &Dom, id: usize, root: bool, out: &mut String) {
    let n = &dom.nodes[id];
    if n.tag == crate::dom::COMMENT { return; }
    if n.tag.is_empty() { out.push_str(&esc(&n.text).replace('"', "\"")); return; }
    let tag = restore(&n.tag, CAMEL_TAGS);
    out.push('<');
    out.push_str(tag);
    if root {
        if dom.attr(id, "xmlns").is_none() { out.push_str(" xmlns=\"http://www.w3.org/2000/svg\""); }
        if dom.attr(id, "xmlns:xlink").is_none() { out.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\""); }
    }
    for (k, v) in &n.attrs { out.push_str(&format!(" {}=\"{}\"", restore(k, CAMEL_ATTRS), esc(v))); }
    out.push('>');
    for &k in &n.kids { write(dom, k, false, out); }
    out.push_str(&format!("</{tag}>"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::parse;

    #[test]
    fn renders_shapes_with_alpha() {
        let im = render("<svg xmlns='http://www.w3.org/2000/svg' width='20' height='10'><rect width='10' height='10' fill='#ff0000'/></svg>").unwrap();
        assert_eq!((im.w, im.h), (20, 10));
        assert_eq!(&im.px[..4], &[255, 0, 0, 255]); // left half red
        assert_eq!(im.px[(19) * 4 + 3], 0); // right half transparent
    }


    #[test]
    fn use_pulls_in_a_sprite_symbol_from_elsewhere_in_the_page() {
        let d = parse("<svg style='display:none'><symbol id=dot viewBox='0 0 2 2'><rect width=2 height=2 fill=#00f /></symbol></svg><p><svg width=8 height=8><use href='#dot' width=8 height=8 /></svg></p>", true);
        let svgs: Vec<usize> = d.descendants().into_iter().filter(|&i| d.nodes[i].tag == "svg").collect();
        let m = to_markup(&d, svgs[1]);
        assert!(m.contains("<defs><symbol id=\"dot\""), "{m}");
        let im = render(&m).unwrap();
        assert_eq!(&im.px[..4], &[0, 0, 255, 255], "the symbol is drawn");
    }
    #[test]
    fn inline_svg_keeps_case_sensitive_names() {
        let d = parse("<p><svg viewBox='0 0 4 4' width=8 height=8><defs><linearGradient id=g><stop offset=0 stop-color=#00f /></linearGradient></defs><circle cx=2 cy=2 r=2 fill=url(#g) /></svg></p>", true);
        let id = d.find("svg").unwrap();
        let m = to_markup(&d, id);
        assert!(m.contains("viewBox=") && m.contains("<linearGradient"), "{m}");
        let im = render(&m).unwrap();
        assert_eq!((im.w, im.h), (8, 8));
        assert!(im.px.chunks(4).any(|p| p[3] == 255)); // circle drawn
    }
}
