// HTML -> DOM tree (arena). Forgiving, not spec-complete: simple implied end tags, depth cap, no foreign content rules.
use serde_json::{Map, Value};

/// Tag of a comment node (not a valid element name, so it cannot clash with a real tag).
pub const COMMENT: &str = "#comment";

#[derive(Default, Clone, Debug)]
pub struct Node {
    /// Empty tag = text node (content in `text`).
    pub tag: String,
    pub text: String,
    pub attrs: Vec<(String, String)>,
    pub kids: Vec<usize>,
    pub parent: usize,
}

#[derive(Clone, Debug)]
pub struct Dom { pub nodes: Vec<Node> }

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr", "param"];
const MAX_DEPTH: usize = 120; // bounds recursion everywhere downstream (JSON, styling)

impl Dom {
    fn new() -> Dom { Dom { nodes: vec![Node { tag: "#root".into(), ..Default::default() }] } }

    pub fn add(&mut self, parent: usize, mut n: Node) -> usize {
        n.parent = parent;
        self.nodes.push(n);
        let id = self.nodes.len() - 1;
        self.nodes[parent].kids.push(id);
        id
    }

    pub fn attr(&self, id: usize, name: &str) -> Option<&str> {
        self.nodes[id].attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// All node ids in document order.
    pub fn descendants(&self) -> Vec<usize> {
        let (mut out, mut st) = (vec![], vec![0usize]);
        while let Some(id) = st.pop() {
            out.push(id);
            st.extend(self.nodes[id].kids.iter().rev());
        }
        out
    }

    pub fn text_content(&self, id: usize) -> String {
        let n = &self.nodes[id];
        if n.tag.is_empty() { return n.text.clone(); }
        n.kids.iter().map(|&k| self.text_content(k)).collect()
    }

    pub fn find(&self, tag: &str) -> Option<usize> { self.descendants().into_iter().find(|&i| self.nodes[i].tag == tag) }

    fn detached(&mut self, tag: &str) -> usize {
        self.nodes.push(Node { tag: tag.into(), ..Default::default() });
        self.nodes.len() - 1
    }

    fn move_to(&mut self, kid: usize, parent: usize) {
        self.nodes[kid].parent = parent;
        self.nodes[parent].kids.push(kid);
    }

    /// Guarantee root > html > [head, body].
    fn normalize(&mut self) {
        let root_kids = std::mem::take(&mut self.nodes[0].kids);
        let html = root_kids.iter().copied().find(|&k| self.nodes[k].tag == "html").unwrap_or_else(|| self.detached("html"));
        self.nodes[html].parent = 0;
        self.nodes[0].kids = vec![html];
        for k in root_kids { if k != html { self.move_to(k, html); } }
        let kids = std::mem::take(&mut self.nodes[html].kids);
        let head = kids.iter().copied().find(|&k| self.nodes[k].tag == "head").unwrap_or_else(|| self.detached("head"));
        let body = kids.iter().copied().find(|&k| self.nodes[k].tag == "body").unwrap_or_else(|| self.detached("body"));
        self.nodes[html].kids = vec![head, body];
        self.nodes[head].parent = html;
        self.nodes[body].parent = html;
        for k in kids { if k != head && k != body { self.move_to(k, body); } }
    }

    /// Removes comment nodes from the tree the page is laid out from (they stay in the arena, so node numbers do not move) and joins the text around them.
    pub fn strip_comments(&mut self) {
        for id in 0..self.nodes.len() {
            if !self.nodes[id].kids.iter().any(|&k| self.nodes[k].tag == COMMENT) { continue; }
            let mut out: Vec<usize> = vec![];
            for k in std::mem::take(&mut self.nodes[id].kids) {
                if self.nodes[k].tag == COMMENT { continue; }
                match out.last() {
                    Some(&l) if self.nodes[l].tag.is_empty() && self.nodes[k].tag.is_empty() => { let t = std::mem::take(&mut self.nodes[k].text); self.nodes[l].text.push_str(&t); }
                    _ => out.push(k),
                }
            }
            self.nodes[id].kids = out;
        }
    }

    pub fn has_comments(&self) -> bool { self.nodes.iter().any(|n| n.tag == COMMENT) }

    pub fn to_json(&self, id: usize) -> Value {
        let n = &self.nodes[id];
        if n.tag.is_empty() { return Value::String(n.text.clone()); }
        if n.tag == COMMENT { return serde_json::json!({ "m": n.text }); }
        let a: Map<String, Value> = n.attrs.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        let mut m = Map::new();
        m.insert("t".into(), Value::String(n.tag.clone()));
        m.insert("a".into(), Value::Object(a));
        m.insert("c".into(), Value::Array(n.kids.iter().map(|&k| self.to_json(k)).collect()));
        Value::Object(m)
    }

    /// Build a DOM whose single child of the root is `v` (what `document.documentElement` serializes to).
    pub fn from_json(v: &Value) -> Option<Dom> {
        let mut dom = Dom::new();
        add_json(&mut dom, 0, v)?;
        Some(dom)
    }
}

fn add_json(dom: &mut Dom, parent: usize, v: &Value) -> Option<()> {
    match v {
        Value::String(s) => { dom.add(parent, Node { text: s.clone(), ..Default::default() }); }
        Value::Object(m) if m.contains_key("m") => { dom.add(parent, Node { tag: COMMENT.into(), text: m["m"].as_str().unwrap_or("").to_string(), ..Default::default() }); }
        Value::Object(m) => {
            let attrs = m.get("a")?.as_object()?.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect();
            let id = dom.add(parent, Node { tag: m.get("t")?.as_str()?.to_string(), attrs, ..Default::default() });
            for c in m.get("c")?.as_array()? { add_json(dom, id, c)?; }
        }
        _ => return None,
    }
    Some(())
}

/// JSON array of the nodes `html` parses to, for innerHTML.
pub fn fragment_json(html: &str) -> String {
    let dom = parse(html, false);
    Value::Array(dom.nodes[0].kids.iter().map(|&k| dom.to_json(k)).collect()).to_string()
}

fn tag_end(s: &str, from: usize) -> usize {
    let b = s.as_bytes();
    let (mut i, mut q, mut prev) = (from + 1, 0u8, 0u8);
    while i < b.len() {
        match (q, b[i]) {
            // a quote only opens a value right after `=`; a stray one (`data-id="x"">`) is part of an attribute name, as in browsers
            (0, b'"' | b'\'') if prev == b'=' => q = b[i],
            (0, b'>') => return i,
            (x, y) if x != 0 && x == y => q = 0,
            _ => {}
        }
        if !b[i].is_ascii_whitespace() { prev = b[i]; }
        i += 1;
    }
    b.len()
}

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    hay.as_bytes().windows(needle.len()).position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let (b, mut i, mut out): (&[u8], usize, Vec<(String, String)>) = (s.as_bytes(), 0, vec![]);
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') { i += 1; }
        if i >= b.len() { break; }
        let s0 = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' { i += 1; }
        let name = s[s0..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
        let mut val = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                let v0 = i;
                while i < b.len() && b[i] != q { i += 1; }
                val = decode(&s[v0..i]);
                i = (i + 1).min(b.len());
            } else {
                let v0 = i;
                while i < b.len() && !b[i].is_ascii_whitespace() { i += 1; }
                val = decode(&s[v0..i]);
            }
        }
        if i == s0 { i += 1; }
        if !name.is_empty() && !out.iter().any(|(k, _)| *k == name) { out.push((name, val)); }
    }
    out
}

pub fn decode(s: &str) -> String {
    if !s.contains('&') { return s.into(); }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let ent = rest.find(';').filter(|&e| e <= 10).map(|e| &rest[1..e]);
        let ch = ent.and_then(|e| Some(match e {
            "amp" => '&', "lt" => '<', "gt" => '>', "quot" => '"', "apos" => '\'', "nbsp" => ' ', "mdash" => '—', "ndash" => '–',
            "hellip" => '…', "copy" => '©', "reg" => '®', "laquo" => '«', "raquo" => '»', "rsaquo" => '›', "lsaquo" => '‹',
            "middot" => '·', "bull" => '•', "times" => '×', "larr" => '←', "rarr" => '→', "euro" => '€',
            _ => {
                let n = e.strip_prefix('#')?;
                char::from_u32(match n.strip_prefix(['x', 'X']) { Some(h) => u32::from_str_radix(h, 16).ok()?, None => n.parse().ok()? })?
            }
        }));
        match (ch, ent) {
            (Some(c), Some(e)) => { out.push(c); rest = &rest[e.len() + 2..]; }
            _ => { out.push('&'); rest = &rest[1..]; }
        }
    }
    out + rest
}

fn implied_close(dom: &Dom, stack: &mut Vec<usize>, name: &str) {
    let (targets, stops): (&[&str], &[&str]) = match name {
        "li" => (&["li"], &["ul", "ol"]),
        "dt" | "dd" => (&["dt", "dd"], &["dl"]),
        "td" | "th" => (&["td", "th"], &["tr", "table"]),
        "tr" => (&["tr"], &["table"]),
        "option" => (&["option"], &["select"]),
        "p" | "div" | "ul" | "ol" | "dl" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "table" | "form" | "pre" | "blockquote" | "section"
        | "article" | "header" | "footer" | "nav" | "main" | "aside" | "hr" | "fieldset" => {
            if stack.len() > 1 && dom.nodes[*stack.last().unwrap()].tag == "p" { stack.pop(); }
            return;
        }
        _ => return,
    };
    for k in (1..stack.len()).rev() {
        let tag = dom.nodes[stack[k]].tag.as_str();
        if targets.contains(&tag) { stack.truncate(k); return; }
        if stops.contains(&tag) { return; }
    }
}

fn push_text(dom: &mut Dom, stack: &[usize], text: &str, doc: bool) {
    let p = *stack.last().unwrap();
    if doc && p == 0 && text.trim().is_empty() { return; }
    if let Some(&l) = dom.nodes[p].kids.last() {
        if dom.nodes[l].tag.is_empty() { dom.nodes[l].text.push_str(text); return; }
    }
    dom.add(p, Node { text: text.into(), ..Default::default() });
}

/// `doc`: wrap into html/head/body (documents); false for fragments.
pub fn parse(html: &str, doc: bool) -> Dom {
    let mut dom = Dom::new();
    let mut stack = vec![0usize];
    let mut i = 0;
    while i < html.len() {
        if html.as_bytes()[i] == b'<' {
            let rest = &html[i..];
            if rest.starts_with("<!--") {
                let end = rest[4..].find("-->").map(|e| 4 + e);
                // kept for scripts (hydration looks for <!--[--> anchors); a document's root-level comments are dropped so node numbering stays that of <html>
                if !(doc && stack.len() == 1) { dom.add(*stack.last().unwrap(), Node { tag: COMMENT.into(), text: rest[4..end.unwrap_or(rest.len())].into(), ..Default::default() }); }
                i = end.map_or(html.len(), |e| i + e + 3);
                continue;
            }
            if rest.starts_with("<?") {
                // `<?x>` is a bogus comment whose data is `?x` (Lit marks its child parts this way)
                let end = rest.find('>').unwrap_or(rest.len());
                if !(doc && stack.len() == 1) { dom.add(*stack.last().unwrap(), Node { tag: COMMENT.into(), text: rest[1..end].into(), ..Default::default() }); }
                i = (i + end + 1).min(html.len());
                continue;
            }
            if rest.starts_with("<!") || rest.starts_with("<?") { i = rest.find('>').map_or(html.len(), |e| i + e + 1); continue; }
            let closing = rest.starts_with("</");
            let first = rest.as_bytes().get(if closing { 2 } else { 1 }).copied().unwrap_or(0);
            if !first.is_ascii_alphabetic() { push_text(&mut dom, &stack, "<", doc); i += 1; continue; }
            let end = tag_end(html, i);
            let inner = &html[i + 1 + closing as usize..end];
            i = (end + 1).min(html.len());
            let nlen = inner.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(inner.len());
            let name = inner[..nlen].to_ascii_lowercase();
            if closing {
                if let Some(k) = (1..stack.len()).rev().find(|&k| dom.nodes[stack[k]].tag == name) { stack.truncate(k); }
                continue;
            }
            implied_close(&dom, &mut stack, &name);
            let void = VOID.contains(&name.as_str());
            let id = dom.add(*stack.last().unwrap(), Node { tag: name.clone(), attrs: parse_attrs(&inner[nlen..]), ..Default::default() });
            if void || stack.len() >= MAX_DEPTH { continue; }
            stack.push(id);
            if matches!(name.as_str(), "script" | "style" | "title" | "textarea") {
                let j = find_ci(&html[i..], &format!("</{name}")).map_or(html.len(), |e| i + e);
                if j > i {
                    let raw = &html[i..j];
                    let text = if name == "script" || name == "style" { raw.to_string() } else { decode(raw) };
                    dom.add(id, Node { text, ..Default::default() });
                }
                i = j;
            }
        } else {
            let end = html[i..].find('<').map_or(html.len(), |e| i + e);
            push_text(&mut dom, &stack, &decode(&html[i..end]), doc);
            i = end;
        }
    }
    if doc { dom.normalize(); }
    dom
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(d: &Dom) -> Vec<String> { d.descendants().into_iter().skip(1).map(|i| if d.nodes[i].tag.is_empty() { format!("'{}'", d.nodes[i].text) } else { d.nodes[i].tag.clone() }).collect() }

    #[test]
    fn stray_quote_after_a_value_does_not_swallow_the_next_tags() {
        let d = parse("<ul><li class=\"\" data-id=\"x\"\"><b>hi</b></li></ul>", true);
        let li = d.find("li").unwrap();
        assert_eq!(d.attr(li, "data-id").as_deref(), Some("x"));
        assert_eq!(d.text_content(d.find("b").unwrap()), "hi");
    }

    #[test]
    fn processing_instruction_is_a_comment() {
        let d = parse("<p>a<?lit$1$>b</p>", true);
        let kids = &d.nodes[d.find("p").unwrap()].kids;
        assert_eq!((d.nodes[kids[1]].tag.as_str(), d.nodes[kids[1]].text.as_str()), (COMMENT, "?lit$1$"));
    }

    #[test]
    fn fragments_keep_root_whitespace() {
        // Svelte templates are `<!> <main>..</main> <!>`: sibling walks count the spaces
        assert_eq!(parse("<!----> <main></main> <b></b>", false).nodes[0].kids.len(), 5);
        assert_eq!(parse(" <p></p> ", true).nodes[0].kids.len(), 1, "documents still drop it");
    }

    #[test]
    fn comments_reach_scripts_but_not_layout() {
        let d = parse("<!-- root --><p>a<!--[-->b<!--]--></p>", true);
        let p = d.find("p").unwrap();
        assert_eq!(d.nodes[p].kids.iter().filter(|&&k| d.nodes[k].tag == COMMENT).count(), 2, "kept inside elements");
        assert!(d.nodes[0].kids.iter().all(|&k| d.nodes[k].tag != COMMENT), "root-level comments are dropped");
        let d2 = Dom::from_json(&d.to_json(d.nodes[0].kids[0])).unwrap();
        assert!(d2.has_comments(), "survives the JSON bridge");
        let mut d3 = d2.clone();
        d3.strip_comments();
        let p3 = d3.find("p").unwrap();
        assert_eq!(d3.nodes[p3].kids.len(), 1, "text around the comments is joined");
        assert_eq!(d3.text_content(p3), "ab");
    }

    #[test]
    fn structure_and_implied_ends() {
        let d = parse("<ul><li>a<li>b</ul><p>x<p>y<b>z</b>", true);
        assert_eq!(tags(&d).join(" "), "html head body ul li 'a' li 'b' p 'x' p 'y' b 'z'");
    }

    #[test]
    fn raw_text_and_attrs() {
        let d = parse("<a href=/x class='c d' data-q=\"a>b\">t</a><script>if(a<b){}</script>", true);
        let a = d.find("a").unwrap();
        assert_eq!(d.attr(a, "href"), Some("/x"));
        assert_eq!(d.attr(a, "data-q"), Some("a>b"));
        assert_eq!(d.text_content(d.find("script").unwrap()), "if(a<b){}");
    }

    #[test]
    fn json_roundtrip() {
        let d = parse("<p id=x>hi <b>there</b></p>", true);
        let v = d.to_json(d.nodes[0].kids[0]);
        let d2 = Dom::from_json(&v).unwrap();
        assert_eq!(tags(&d), tags(&d2));
        assert_eq!(d2.attr(d2.find("p").unwrap(), "id"), Some("x"));
    }

    #[test]
    fn entities() {
        assert_eq!(decode("a&nbsp;b &#65;&#x42; &mdash; &bogus; & x"), "a b AB — &bogus; & x");
    }

    #[test]
    fn deep_nesting_is_capped() {
        let d = parse(&"<div>".repeat(5000), true);
        assert!(d.nodes.len() < 6000);
    }
}
