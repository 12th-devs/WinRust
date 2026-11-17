// Fonts, line layout and software drawing (pixels are 0x00RRGGBB in a softbuffer slice).
use crate::{load::Img, render::{Field, FieldKind}};
use fontdue::{Font, FontSettings, Metrics};
use std::{cell::Cell, collections::HashMap, sync::Mutex};

thread_local! { static CLIP: Cell<Option<(i32, i32, i32, i32)>> = const { Cell::new(None) }; }

/// Restricts all painting to the screen rectangle (x0, y0, x1, y1), used for the content of a scroll container; None = the whole buffer.
pub fn set_clip(c: Option<(i32, i32, i32, i32)>) { CLIP.with(|k| k.set(c)); }

fn clip_box(bw: usize, bh: usize) -> (i32, i32, i32, i32) {
    let (a, b, c, d) = CLIP.with(Cell::get).unwrap_or((0, 0, i32::MAX, i32::MAX));
    (a.max(0), b.max(0), c.min(bw as i32), d.min(bh as i32))
}

/// A text face packs weight/style/family: bit 0 bold, bit 1 italic, bit 2 monospace, bits 4.. web-font family id (0 = none).
pub const BOLD: u16 = 1;
pub const MONO: u16 = 4;

pub fn face(bold: bool, italic: bool, mono: bool, fam: u16) -> u16 { bold as u16 | (italic as u16) << 1 | (mono as u16) << 2 | fam << 4 }

/// Stable id (1-based) of a lowercase font-family name; shared by the page-loading thread and the UI thread.
pub fn family_id(name: &str) -> u16 {
    static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let Ok(mut n) = NAMES.lock() else { return 0 };
    let lower = name.trim().to_ascii_lowercase();
    match n.iter().position(|x| *x == lower) { Some(i) => i as u16 + 1, None => { n.push(lower); n.len().min(4095) as u16 } }
}

/// Rasterizer behind a font slot. Fontdue parses every outline of a font when it is loaded (a 20 MB CJK font costs 150-250 MB),
/// so coverage fallbacks use ttf-parser instead: the file bytes stay as they are and each glyph is outlined only when it is drawn.
enum Rast { Fd(Font), Tp(ttf_parser::Face<'static>) }

#[derive(Clone)]
enum Bytes { Own(std::sync::Arc<Vec<u8>>), Leaked(&'static [u8]) }

impl Bytes { fn as_slice(&self) -> &[u8] { match self { Bytes::Own(a) => a, Bytes::Leaked(s) => s } } }

struct Slot { font: Rast, bold: bool, data: Bytes }

impl Slot {
    fn has(&self, c: char) -> bool {
        match &self.font { Rast::Fd(f) => f.lookup_glyph_index(c) != 0, Rast::Tp(f) => f.glyph_index(c).is_some_and(|g| g.0 != 0) }
    }
    fn raster(&self, c: char, size: f32) -> (Metrics, Vec<u8>) {
        match &self.font { Rast::Fd(f) => f.rasterize(c, size), Rast::Tp(f) => tp_raster(f, f.glyph_index(c).map_or(0, |g| g.0), size) }
    }
    fn raster_id(&self, id: u16, size: f32) -> (Metrics, Vec<u8>) {
        match &self.font { Rast::Fd(f) => f.rasterize_indexed(id, size), Rast::Tp(f) => tp_raster(f, id, size) }
    }
}

/// Collects a glyph outline into an `ab_glyph_rasterizer` canvas (font units, y up -> pixels, y down).
struct Outline { r: ab_glyph_rasterizer::Rasterizer, k: f32, ox: f32, top: f32, start: (f32, f32), last: (f32, f32) }

impl Outline {
    fn pt(&self, x: f32, y: f32) -> (f32, f32) { (x * self.k - self.ox, self.top - y * self.k) }
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) { self.start = self.pt(x, y); self.last = self.start; }
    fn line_to(&mut self, x: f32, y: f32) { let p = self.pt(x, y); self.r.draw_line(ab_glyph_rasterizer::point(self.last.0, self.last.1), ab_glyph_rasterizer::point(p.0, p.1)); self.last = p; }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (c, p) = (self.pt(x1, y1), self.pt(x, y));
        self.r.draw_quad(ab_glyph_rasterizer::point(self.last.0, self.last.1), ab_glyph_rasterizer::point(c.0, c.1), ab_glyph_rasterizer::point(p.0, p.1));
        self.last = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (c1, c2, p) = (self.pt(x1, y1), self.pt(x2, y2), self.pt(x, y));
        self.r.draw_cubic(ab_glyph_rasterizer::point(self.last.0, self.last.1), ab_glyph_rasterizer::point(c1.0, c1.1), ab_glyph_rasterizer::point(c2.0, c2.1), ab_glyph_rasterizer::point(p.0, p.1));
        self.last = p;
    }
    fn close(&mut self) {
        if self.last != self.start { self.r.draw_line(ab_glyph_rasterizer::point(self.last.0, self.last.1), ab_glyph_rasterizer::point(self.start.0, self.start.1)); }
        self.last = self.start;
    }
}

/// One glyph as fontdue-style metrics and an 8-bit coverage bitmap, outlined on demand.
fn tp_raster(face: &ttf_parser::Face<'static>, id: u16, size: f32) -> (Metrics, Vec<u8>) {
    let gid = ttf_parser::GlyphId(id);
    let k = size / face.units_per_em().max(1) as f32;
    let mut m = Metrics { advance_width: face.glyph_hor_advance(gid).unwrap_or(0) as f32 * k, ..Metrics::default() };
    let Some(bb) = face.glyph_bounding_box(gid) else { return (m, vec![]) };
    let (x0, x1, y0, y1) = ((bb.x_min as f32 * k).floor(), (bb.x_max as f32 * k).ceil(), (bb.y_min as f32 * k).floor(), (bb.y_max as f32 * k).ceil());
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    if w == 0 || h == 0 || w * h > 4_000_000 { return (m, vec![]); }
    let mut ob = Outline { r: ab_glyph_rasterizer::Rasterizer::new(w, h), k, ox: x0, top: y1, start: (0.0, 0.0), last: (0.0, 0.0) };
    if face.outline_glyph(gid, &mut ob).is_none() { return (m, vec![]); }
    let mut bm = vec![0u8; w * h];
    ob.r.for_each_pixel_2d(|x, y, a| bm[y as usize * w + x as usize] = (a * 255.0).round().clamp(0.0, 255.0) as u8);
    (m.xmin, m.ymin, m.width, m.height) = (x0 as i32, y0 as i32, w, h);
    (m, bm)
}

/// A fallback font file: path, whether it is loaded as a slot, and (once scanned) the code point ranges its cmap covers,
/// so that fonts known not to cover a character are not read again.
struct Fb { path: std::path::PathBuf, loaded: bool, cov: Option<Vec<(u32, u32)>> }

fn covers(ranges: &[(u32, u32)], c: u32) -> bool { ranges.binary_search_by(|&(a, b)| if c < a { std::cmp::Ordering::Greater } else if c > b { std::cmp::Ordering::Less } else { std::cmp::Ordering::Equal }).is_ok() }

fn coverage(face: &ttf_parser::Face) -> Vec<(u32, u32)> {
    let mut cps: Vec<u32> = vec![];
    if let Some(cmap) = face.tables().cmap { for st in cmap.subtables { if st.is_unicode() { st.codepoints(|c| cps.push(c)); } } }
    cps.sort_unstable();
    cps.dedup();
    let mut out: Vec<(u32, u32)> = vec![];
    for c in cps { match out.last_mut() { Some(l) if l.1 + 1 == c => l.1 = c, _ => out.push((c, c)) } }
    out
}

/// Four faces (regular, bold, italic, bold-italic) of one family; a face is read from disk the first time text needs it.
struct LazySet { paths: [String; 4], slots: [Option<usize>; 4] }

impl LazySet { fn none() -> LazySet { LazySet { paths: Default::default(), slots: [None; 4] } } }

/// Shaped text run: glyph ids with pixel advances/offsets in visual (left-to-right) order.
struct Shaped { glyphs: Vec<(u16, f32, f32, f32)>, width: f32, slot: usize }

/// Scripts that need shaping (joining, reordering, marks): Hebrew, Arabic, Indic, Thai, presentation forms.
pub fn complex(s: &str) -> bool { s.chars().any(|c| matches!(c as u32, 0x0590..=0x08FF | 0x0900..=0x0EFF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)) }

/// Right-to-left text: the first strong character is Hebrew or Arabic.
pub fn is_rtl(s: &str) -> bool { s.chars().find(|c| c.is_alphabetic()).is_some_and(|c| matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)) }

/// Loaded fonts: regular/bold/italic/bold-italic sets for sans and mono, lazily loaded coverage fallbacks (symbols, CJK, ...)
/// and per-page web fonts. Glyphs are looked up per character, so a missing glyph falls through to the next font.
pub struct Fonts {
    slots: Vec<Slot>,
    sans: LazySet,
    mono: LazySet,
    fallback: Vec<Fb>,
    fb_slots: Vec<usize>,                      // slots of loaded fallbacks
    missing: std::collections::HashSet<char>,  // characters no font covers (not searched again)
    web: HashMap<u16, [Option<usize>; 4]>,
    cache: HashMap<(char, u32, u16), (Metrics, Vec<u8>, bool)>,
    shaped: HashMap<(String, u32, u16), std::rc::Rc<Shaped>>,
    glyphs: HashMap<(usize, u16, u32), (Metrics, Vec<u8>)>,
}

const SANS: &[[&str; 4]] = &[
    ["C:\\Windows\\Fonts\\segoeui.ttf", "C:\\Windows\\Fonts\\segoeuib.ttf", "C:\\Windows\\Fonts\\segoeuii.ttf", "C:\\Windows\\Fonts\\segoeuiz.ttf"],
    ["C:\\Windows\\Fonts\\arial.ttf", "C:\\Windows\\Fonts\\arialbd.ttf", "C:\\Windows\\Fonts\\ariali.ttf", "C:\\Windows\\Fonts\\arialbi.ttf"],
    ["/System/Library/Fonts/Supplemental/Arial.ttf", "/System/Library/Fonts/Supplemental/Arial Bold.ttf", "/System/Library/Fonts/Supplemental/Arial Italic.ttf", "/System/Library/Fonts/Supplemental/Arial Bold Italic.ttf"],
    ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-BoldOblique.ttf"],
    ["/usr/share/fonts/TTF/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf", "/usr/share/fonts/TTF/DejaVuSans-Oblique.ttf", "/usr/share/fonts/TTF/DejaVuSans-BoldOblique.ttf"],
];
const MONOS: &[[&str; 4]] = &[
    ["C:\\Windows\\Fonts\\consola.ttf", "C:\\Windows\\Fonts\\consolab.ttf", "C:\\Windows\\Fonts\\consolai.ttf", "C:\\Windows\\Fonts\\consolaz.ttf"],
    ["C:\\Windows\\Fonts\\cour.ttf", "C:\\Windows\\Fonts\\courbd.ttf", "C:\\Windows\\Fonts\\couri.ttf", "C:\\Windows\\Fonts\\courbi.ttf"],
    ["/System/Library/Fonts/Menlo.ttc", "", "", ""],
    ["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Oblique.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-BoldOblique.ttf"],
    ["/usr/share/fonts/TTF/DejaVuSansMono.ttf", "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf", "/usr/share/fonts/TTF/DejaVuSansMono-Oblique.ttf", "/usr/share/fonts/TTF/DejaVuSansMono-BoldOblique.ttf"],
];
/// Tried in order, only when a character is missing from the main fonts (large CJK fonts are therefore loaded on demand).
const FALLBACKS: &[&str] = &[
    "C:\\Windows\\Fonts\\seguisym.ttf", "C:\\Windows\\Fonts\\tahoma.ttf", "C:\\Windows\\Fonts\\Nirmala.ttf", "C:\\Windows\\Fonts\\Nirmala.ttc", "C:\\Windows\\Fonts\\msyh.ttc", "C:\\Windows\\Fonts\\YuGothM.ttc",
    "C:\\Windows\\Fonts\\malgun.ttf", "C:\\Windows\\Fonts\\simsun.ttc", "C:\\Windows\\Fonts\\seguiemj.ttf", "C:\\Windows\\Fonts\\mingliub.ttc",
    "/System/Library/Fonts/PingFang.ttc", "/System/Library/Fonts/Apple Symbols.ttf", "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf", "/usr/share/fonts/truetype/lohit-devanagari/Lohit-Devanagari.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
];

fn variant(set: &[Option<usize>; 4], ix: usize) -> Option<usize> { set[ix].or(set[ix & 1]).or(set[0]).or_else(|| set.iter().flatten().next().copied()) }

fn read_font(path: &str) -> Option<(Font, Vec<u8>)> {
    if path.is_empty() { return None; }
    let data = std::fs::read(path).ok()?;
    Some((Font::from_bytes(data.as_slice(), FontSettings::default()).ok()?, data))
}

impl Fonts {
    /// A single regular font (tests, tools).
    #[cfg(test)]
    pub fn new(font: Font, mono: Option<Font>) -> Fonts {
        let mut f = Fonts { slots: vec![], sans: LazySet::none(), mono: LazySet::none(), fallback: vec![], web: HashMap::new(), cache: HashMap::new(), shaped: HashMap::new(), glyphs: HashMap::new(), fb_slots: vec![], missing: Default::default() };
        f.sans.slots[0] = Some(f.add(font, false, vec![]));
        if let Some(m) = mono { f.mono.slots[0] = Some(f.add(m, false, vec![])); }
        f
    }

    /// The first installed sans and mono families we know (regular faces now, the rest on demand) plus lazy fallbacks.
    pub fn system() -> Option<Fonts> {
        let mut f = Fonts { slots: vec![], sans: LazySet::none(), mono: LazySet::none(), fallback: FALLBACKS.iter().map(|p| Fb { path: p.into(), loaded: false, cov: None }).collect(), web: HashMap::new(), cache: HashMap::new(), shaped: HashMap::new(), glyphs: HashMap::new(), fb_slots: vec![], missing: Default::default() };
        for (set, lists) in [(0, SANS), (1, MONOS)] {
            for paths in lists {
                let Some((font, data)) = read_font(paths[0]) else { continue };
                let reg = f.add(font, false, data);
                let ls = LazySet { paths: [String::new(), paths[1].into(), paths[2].into(), paths[3].into()], slots: [Some(reg), None, None, None] };
                if set == 0 { f.sans = ls } else { f.mono = ls }
                break;
            }
        }
        f.sans.slots[0]?;
        Some(f)
    }

    /// Loads face `ix` of a set when it exists on disk and was not read yet.
    fn ensure(&mut self, mono: bool, ix: usize) -> Option<usize> {
        let set = if mono { &mut self.mono } else { &mut self.sans };
        if set.slots[ix].is_some() { return set.slots[ix]; }
        let path = std::mem::take(&mut set.paths[ix]); // taken: a missing or broken file is not retried
        if path.is_empty() { return None; }
        let (font, data) = read_font(&path)?;
        let i = self.add(font, ix & 1 != 0, data);
        if mono { self.mono.slots[ix] = Some(i) } else { self.sans.slots[ix] = Some(i) }
        Some(i)
    }

    /// Best available face for (bold, italic): exact, then same weight upright, then regular, then anything loaded.
    fn pick_face(&mut self, mono: bool, ix: usize) -> Option<usize> {
        for want in [ix, ix & 1, 0] { if let Some(i) = self.ensure(mono, want) { return Some(i); } }
        let set = if mono { &self.mono } else { &self.sans };
        set.slots.iter().flatten().next().copied()
    }

    fn add(&mut self, font: Font, bold: bool, data: Vec<u8>) -> usize { self.slots.push(Slot { font: Rast::Fd(font), bold, data: Bytes::Own(std::sync::Arc::new(data)) }); self.slots.len() - 1 }

    /// Registers a web font (already decoded to TTF/OTF bytes) for family id `fam`; false when it does not parse.
    pub fn add_web(&mut self, fam: u16, bold: bool, italic: bool, data: &[u8]) -> bool {
        if self.slots.len() > 64 { return false; }
        let Ok(font) = Font::from_bytes(data, FontSettings::default()) else { return false };
        let i = self.add(font, bold, data.to_vec());
        self.web.entry(fam).or_default()[bold as usize | (italic as usize) << 1] = Some(i);
        self.cache.retain(|k, _| k.2 >> 4 != fam);
        true
    }

    /// Font slot and "needs fake bold" for character `c` in `face`.
    fn pick(&mut self, c: char, face: u16) -> (usize, bool) {
        let ix = (face & 3) as usize;
        let web = (face >> 4 != 0).then(|| self.web.get(&(face >> 4)).and_then(|s| variant(s, ix))).flatten();
        let main = if face & MONO != 0 { self.pick_face(true, ix).or_else(|| self.pick_face(false, ix)) } else { self.pick_face(false, ix) }.unwrap_or(0);
        let has = |s: &Vec<Slot>, i: usize| s[i].has(c);
        let mut idx = web.filter(|&i| has(&self.slots, i)).or_else(|| has(&self.slots, main).then_some(main));
        if idx.is_none() && !c.is_control() && !self.missing.contains(&c) {
            if self.fallback.is_empty() { idx = (0..self.slots.len()).find(|&i| has(&self.slots, i)); } // plain test fonts
            // fonts loaded earlier as fallbacks first, then unloaded candidates: only one that covers `c` is kept in memory
            idx = idx.or_else(|| self.fb_slots.iter().copied().find(|&i| has(&self.slots, i)));
            for k in 0..self.fallback.len() {
                if idx.is_some() { break; }
                if self.fallback[k].loaded || self.fallback[k].cov.as_ref().is_some_and(|r| !covers(r, c as u32)) { continue; }
                let Ok(data) = std::fs::read(&self.fallback[k].path) else { self.fallback[k].loaded = true; continue }; // unreadable: never retried
                let cov = match ttf_parser::Face::parse(&data, 0) { Ok(face) => coverage(&face), Err(_) => { self.fallback[k].loaded = true; continue } };
                if covers(&cov, c as u32) {
                    let leaked: &'static [u8] = Box::leak(data.into_boxed_slice()); // font files live as long as the process
                    let Ok(face) = ttf_parser::Face::parse(leaked, 0) else { self.fallback[k].loaded = true; continue };
                    if std::env::var_os("CHROME_RS_DEBUG").is_some() { eprintln!("fallback font {} loaded for {c:?} (U+{:04X}), {} bytes", self.fallback[k].path.display(), c as u32, leaked.len()); }
                    self.slots.push(Slot { font: Rast::Tp(face), bold: false, data: Bytes::Leaked(leaked) });
                    self.fallback[k].loaded = true;
                    self.fb_slots.push(self.slots.len() - 1);
                    idx = Some(self.slots.len() - 1);
                } else { self.fallback[k].cov = Some(cov); }
            }
            if idx.is_none() { self.missing.insert(c); }
        }
        let i = idx.unwrap_or(main);
        (i, face & BOLD != 0 && !self.slots[i].bold)
    }

    fn get(&mut self, c: char, size: f32, face: u16) -> &(Metrics, Vec<u8>, bool) {
        let key = (c, size as u32, face);
        if !self.cache.contains_key(&key) {
            if self.cache.len() > 40_000 { self.cache.clear(); }
            let (i, synth) = self.pick(c, face);
            let (m, b) = self.slots[i].raster(c, size);
            self.cache.insert(key, (m, b, synth));
        }
        &self.cache[&key]
    }

    pub fn width(&mut self, s: &str, size: f32) -> f32 { self.width_f(s, size, 0) }
    pub fn width_f(&mut self, s: &str, size: f32, face: u16) -> f32 {
        if complex(s) { if let Some(sh) = self.shape(s, size, face) { return sh.width; } }
        s.chars().map(|c| self.get(c, size, face).0.advance_width).sum()
    }

    /// Shapes `s` with the font that covers its first complex character (cached); None when the font data cannot be shaped.
    fn shape(&mut self, s: &str, size: f32, face: u16) -> Option<std::rc::Rc<Shaped>> {
        let key = (s.to_string(), size as u32, face);
        if let Some(r) = self.shaped.get(&key) { return Some(r.clone()); }
        let first = s.chars().find(|&c| complex(&c.to_string())).unwrap_or('a');
        let (slot, _) = self.pick(first, face);
        let data = self.slots[slot].data.clone();
        let rb = rustybuzz::Face::from_slice(data.as_slice(), 0)?;
        let k = size / rb.units_per_em() as f32;
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(s);
        buf.guess_segment_properties();
        let out = rustybuzz::shape(&rb, &[], buf);
        let glyphs: Vec<(u16, f32, f32, f32)> = out.glyph_infos().iter().zip(out.glyph_positions()).map(|(i, p)| (i.glyph_id as u16, p.x_offset as f32 * k, p.y_offset as f32 * k, p.x_advance as f32 * k)).collect();
        let width = glyphs.iter().map(|g| g.3).sum();
        if self.shaped.len() > 5000 { self.shaped.clear(); }
        let r = std::rc::Rc::new(Shaped { glyphs, width, slot });
        self.shaped.insert(key, r.clone());
        Some(r)
    }

    fn glyph(&mut self, slot: usize, id: u16, size: f32) -> &(Metrics, Vec<u8>) {
        if self.glyphs.len() > 20_000 { self.glyphs.clear(); }
        let sl = &self.slots[slot];
        self.glyphs.entry((slot, id, size as u32)).or_insert_with(|| sl.raster_id(id, size))
    }
    pub fn cw(&mut self, c: char, size: f32) -> f32 { self.get(c, size, 0).0.advance_width }
}

pub fn blend(d: u32, s: u32, a: u32) -> u32 {
    let ch = |sh: u32| (((s >> sh) & 255) * a + ((d >> sh) & 255) * (255 - a)) / 255 << sh;
    ch(16) | ch(8) | ch(0)
}

pub fn fill(buf: &mut [u32], bw: usize, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let (cx0, cy0, cx1, cy1) = clip_box(bw, buf.len() / bw.max(1));
    for py in y.max(cy0)..(y + h).min(cy1) {
        for px in x.max(cx0)..(x + w).min(cx1) {
            if let Some(d) = buf.get_mut(py as usize * bw + px as usize) { *d = color; }
        }
    }
}

/// Like `fill`, blended with `a` (0..=255).
pub fn fill_a(buf: &mut [u32], bw: usize, x: i32, y: i32, w: i32, h: i32, color: u32, a: u32) {
    if a >= 255 { return fill(buf, bw, x, y, w, h, color); }
    let (cx0, cy0, cx1, cy1) = clip_box(bw, buf.len() / bw.max(1));
    for py in y.max(cy0)..(y + h).min(cy1) {
        for px in x.max(cx0)..(x + w).min(cx1) {
            if let Some(d) = buf.get_mut(py as usize * bw + px as usize) { *d = blend(*d, color, a); }
        }
    }
}

/// Rounded rectangle, filled or (ring > 0) an outline `ring` px thick; anti-aliased with a signed distance field.
pub fn draw_rrect(buf: &mut [u32], bw: usize, bh: usize, x: f32, y: f32, w: f32, h: f32, r: f32, ring: f32, color: u32) {
    draw_rrect_a(buf, bw, bh, x, y, w, h, r, ring, &|_, _| color, 255);
}

/// `draw_rrect` with a per-pixel color (gradients; arguments are the pixel's position inside the rectangle, 0..1) and an overall alpha.
pub fn draw_rrect_a(buf: &mut [u32], bw: usize, bh: usize, x: f32, y: f32, w: f32, h: f32, r: f32, ring: f32, color: &dyn Fn(f32, f32) -> u32, alpha: u32) {
    let r = r.min(w.min(h) / 2.0).max(0.0);
    let (cx, cy, hx, hy) = (x + w / 2.0, y + h / 2.0, w / 2.0 - r, h / 2.0 - r);
    let (cx0, cy0, cx1, cy1) = clip_box(bw, bh);
    for py in (y.floor() as i32).max(cy0)..((y + h).ceil() as i32).min(cy1) {
        for px in (x.floor() as i32).max(cx0)..((x + w).ceil() as i32).min(cx1) {
            let (qx, qy) = ((px as f32 + 0.5 - cx).abs() - hx, (py as f32 + 0.5 - cy).abs() - hy);
            let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
            let mut cov = (0.5 - d).clamp(0.0, 1.0);
            if ring > 0.0 { cov -= (0.5 - (d + ring)).clamp(0.0, 1.0); }
            if cov > 0.0 { let i = py as usize * bw + px as usize; let c = color((px as f32 + 0.5 - x) / w.max(1.0), (py as f32 + 0.5 - y) / h.max(1.0)); buf[i] = blend(buf[i], c, (cov * alpha as f32) as u32); }
        }
    }
}

/// Paints a layout rectangle (`dy` = vertical offset of the viewport).
pub fn draw_rect(buf: &mut [u32], bw: usize, bh: usize, r: &crate::layout::Rect, dy: f32) {
    let y = r.y + dy;
    if r.a == 0 || r.pic.is_some() { return; } // markers are invisible // background images are drawn by `draw_pic`, which needs the page's pictures
    if let Some(g) = &r.grad {
        let (sin, cos) = g.angle.to_radians().sin_cos();
        let half = (w_h_len(r.w, r.h, sin, cos)) / 2.0;
        let at = |px: f32, py: f32| {
            if g.radial { // farthest-corner sizing: a circle reaches the far corner, an ellipse keeps the box's proportions
                let (cx, cy) = g.at;
                let (dx, dy) = ((px - cx) * r.w, (py - cy) * r.h);
                let (fx, fy) = (cx.max(1.0 - cx) * r.w, cy.max(1.0 - cy) * r.h);
                let t = if g.circle { dx.hypot(dy) / fx.hypot(fy).max(1.0) } else { (dx / (fx * std::f32::consts::SQRT_2).max(1.0)).hypot(dy / (fy * std::f32::consts::SQRT_2).max(1.0)) };
                return grad_color(&g.stops, t.clamp(0.0, 1.0));
            }
            let (dx, dy) = ((px - 0.5) * r.w, (py - 0.5) * r.h);
            let t = if half > 0.0 { 0.5 + (dx * sin - dy * cos) / (2.0 * half) } else { 0.0 };
            grad_color(&g.stops, t.clamp(0.0, 1.0))
        };
        draw_rrect_a(buf, bw, bh, r.x, y, r.w, r.h, r.r, r.ring, &at, r.a as u32);
    } else if r.r > 0.0 || r.ring > 0.0 { draw_rrect_a(buf, bw, bh, r.x, y, r.w, r.h, r.r, r.ring, &|_, _| r.color, r.a as u32); }
    else { fill_a(buf, bw, r.x as i32, y as i32, r.w as i32, r.h as i32, r.color, r.a as u32); }
}

/// A background image: scaled to the tile size, offset inside the rectangle, repeated per axis, clipped to the rectangle.
pub fn draw_pic(buf: &mut [u32], bw: usize, bh: usize, im: &Img, r: &crate::layout::Rect, dy: f32) {
    let Some(pic) = &r.pic else { return };
    if im.w == 0 || im.h == 0 { return; }
    let y = r.y + dy;
    let (cx0, cy0, cx1, cy1) = clip_box(bw, bh);
    let (x0, x1) = ((r.x.floor() as i32).max(cx0), ((r.x + r.w).ceil() as i32).min(cx1));
    let (y0, y1) = ((y.floor() as i32).max(cy0), ((y + r.h).ceil() as i32).min(cy1));
    for py in y0..y1 {
        let mut ly = py as f32 + 0.5 - (y + pic.oy);
        if pic.ry { ly = ly.rem_euclid(pic.h); } else if ly < 0.0 || ly >= pic.h { continue; }
        let sy = ((ly / pic.h * im.h as f32) as usize).min(im.h as usize - 1);
        for px in x0..x1 {
            let mut lx = px as f32 + 0.5 - (r.x + pic.ox);
            if pic.rx { lx = lx.rem_euclid(pic.w); } else if lx < 0.0 || lx >= pic.w { continue; }
            let sx = ((lx / pic.w * im.w as f32) as usize).min(im.w as usize - 1);
            let p = &im.px[(sy * im.w as usize + sx) * 4..][..4];
            if p[3] == 0 { continue; }
            let d = &mut buf[py as usize * bw + px as usize];
            *d = blend(*d, (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32, p[3] as u32);
        }
    }
}

/// Thin scrollbar on the right edge of a scroll container's box at screen (x, y, w, h).
pub fn draw_box_scrollbar(buf: &mut [u32], bw: usize, bh: usize, x: f32, y: f32, w: f32, h: f32, content_h: f32, offset: f32, sc: f32) {
    let max = (content_h - h).max(0.0);
    if max < 1.0 || h < 24.0 * sc { return; }
    let th = (h * h / content_h).max(20.0 * sc).min(h);
    draw_rrect(buf, bw, bh, x + w - 7.0 * sc, y + offset.clamp(0.0, max) / max * (h - th), 5.0 * sc, th, 2.5 * sc, 0.0, 0x9A9A9A);
}

/// Length of the gradient line of a w x h box for a CSS angle (direction `sin, -cos` in screen space).
fn w_h_len(w: f32, h: f32, sin: f32, cos: f32) -> f32 { (w * sin).abs() + (h * cos).abs() }

fn grad_color(stops: &[(f32, u32)], t: f32) -> u32 {
    let Some(first) = stops.first() else { return 0 };
    if t <= first.0 { return first.1; }
    for w in stops.windows(2) {
        let ((p0, c0), (p1, c1)) = (w[0], w[1]);
        if t <= p1 { return if p1 > p0 { blend(c0, c1, (((t - p0) / (p1 - p0)) * 255.0) as u32) } else { c1 }; }
    }
    stops[stops.len() - 1].1
}

/// Nearest-neighbour blit of an RGBA image scaled to w x h at (x, y); alpha-blended, clipped to the buffer.
pub fn draw_image(buf: &mut [u32], bw: usize, bh: usize, im: &Img, x: f32, y: f32, w: f32, h: f32, alpha: u32) {
    let (cx0, cy0, cx1, cy1) = clip_box(bw, bh);
    let (x0, y0, wi, hi) = (x as i32, y as i32, w.max(1.0) as i32, h.max(1.0) as i32);
    for dy in 0..hi {
        let py = y0 + dy;
        if py < cy0 || py >= cy1 { continue; }
        let sy = (dy as u64 * im.h as u64 / hi as u64) as usize;
        for dx in 0..wi {
            let px = x0 + dx;
            if px < cx0 || px >= cx1 { continue; }
            let sx = (dx as u64 * im.w as u64 / wi as u64) as usize;
            let p = &im.px[(sy * im.w as usize + sx) * 4..][..4];
            if p[3] == 0 { continue; }
            let d = &mut buf[py as usize * bw + px as usize];
            *d = blend(*d, (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32, p[3] as u32 * alpha / 255);
        }
    }
}

pub fn draw_text(buf: &mut [u32], bw: usize, bh: usize, f: &mut Fonts, s: &str, x: f32, base: f32, size: f32, color: u32) {
    draw_text_m(buf, bw, bh, f, s, x, base, size, color, false);
}

pub fn draw_text_m(buf: &mut [u32], bw: usize, bh: usize, f: &mut Fonts, s: &str, x: f32, base: f32, size: f32, color: u32, mono: bool) {
    draw_text_f(buf, bw, bh, f, s, x, base, size, color, face(false, false, mono, 0));
}

/// Draws `s` in `face`; bold is faked by double-striking when the chosen font has no bold variant.
pub fn draw_text_f(buf: &mut [u32], bw: usize, bh: usize, f: &mut Fonts, s: &str, x: f32, base: f32, size: f32, color: u32, face: u16) {
    draw_text_ls(buf, bw, bh, f, s, x, base, size, color, face, 0.0);
}

/// `draw_text_f` with `ls` extra pixels after every character (letter-spacing).
pub fn draw_text_ls(buf: &mut [u32], bw: usize, bh: usize, f: &mut Fonts, s: &str, mut x: f32, base: f32, size: f32, color: u32, face: u16, ls: f32) {
    let (cx0, cy0, cx1, cy1) = clip_box(bw, bh);
    if complex(s) {
        if let Some(sh) = f.shape(s, size, face) {
            for &(id, ox, oy, adv) in &sh.glyphs {
                let (m, bm) = f.glyph(sh.slot, id, size);
                let gx = (x + ox + m.xmin as f32) as i32;
                let gy = (base - oy - m.height as f32 - m.ymin as f32) as i32;
                for row in 0..m.height {
                    let py = gy + row as i32;
                    if py < cy0 || py >= cy1 { continue; }
                    for col in 0..m.width {
                        let px = gx + col as i32;
                        let a = bm[row * m.width + col] as u32;
                        if a == 0 || px < cx0 || px >= cx1 { continue; }
                        let d = &mut buf[py as usize * bw + px as usize];
                        *d = blend(*d, color, a);
                    }
                }
                x += adv + ls;
            }
            return;
        }
    }
    for c in s.chars() {
        let (m, bm, synth) = f.get(c, size, face);
        for pass in 0..1 + *synth as i32 {
            let gx = (x + m.xmin as f32) as i32 + pass;
            let gy = (base - m.height as f32 - m.ymin as f32) as i32;
            for row in 0..m.height {
                let py = gy + row as i32;
                if py < cy0 || py >= cy1 { continue; }
                for col in 0..m.width {
                    let px = gx + col as i32;
                    let a = bm[row * m.width + col] as u32;
                    if a == 0 || px < cx0 || px >= cx1 { continue; }
                    let d = &mut buf[py as usize * bw + px as usize];
                    *d = blend(*d, color, a);
                }
            }
        }
        x += m.advance_width + ls;
    }
}

/// Visual lines of `chars` as [start, end) ranges: hard breaks at '\n', soft breaks when a line would exceed `max`.
pub fn wrap_lines(f: &mut Fonts, chars: &[char], fs: f32, max: f32) -> Vec<(usize, usize)> {
    let (mut out, mut start, mut wsum) = (vec![], 0, 0.0);
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' { out.push((start, i)); start = i + 1; wsum = 0.0; continue; }
        let cw = f.cw(c, fs);
        if wsum + cw > max && i > start { out.push((start, i)); start = i; wsum = 0.0; }
        wsum += cw;
    }
    out.push((start, chars.len()));
    out
}

/// Window [start, end) of a single-line value that fits `max` px and keeps the caret in view.
fn visible(f: &mut Fonts, chars: &[char], caret: usize, fs: f32, max: f32) -> (usize, usize) {
    let (mut start, mut w) = (0, 0.0);
    // widen from the caret backwards while it fits, then fill forwards
    let mut s = caret.min(chars.len());
    while s > 0 { let cw = f.cw(chars[s - 1], fs); if w + cw > max { break; } w += cw; s -= 1; }
    start = start.max(s);
    let mut end = start;
    let mut ew = 0.0;
    while end < chars.len() { let cw = f.cw(chars[end], fs); if ew + cw > max { break; } ew += cw; end += 1; }
    (start, end)
}

/// Draws a form control. `caret` is Some(char index) when the control has keyboard focus.
pub fn draw_field(buf: &mut [u32], bw: usize, bh: usize, f: &mut Fonts, fl: &Field, caret: Option<usize>, sc: f32, x: f32, y: f32, w: f32, h: f32) {
    let focused = caret.is_some();
    let (xi, yi, wi, hi) = (x as i32, y as i32, w as i32, h as i32);
    let (border, bg) = match fl.kind {
        FieldKind::Submit => (0x767676, 0xEFEFEF),
        _ => (if focused { 0x1A73E8 } else { 0x767676 }, 0xFFFFFF),
    };
    fill(buf, bw, xi, yi, wi, hi, border);
    fill(buf, bw, xi + 1, yi + 1, wi - 2, hi - 2, bg);
    let fs = (fl.fs * sc).round().max(8.0);
    let base = y + h * 0.5 + fs * 0.35;
    let ink = 0x111111;
    match fl.kind {
        FieldKind::Check => if fl.checked { let tw = f.cw('x', fs); draw_text(buf, bw, bh, f, "x", x + (w - tw) / 2.0, base, fs, ink); },
        FieldKind::Radio => if fl.checked { let tw = f.cw('\u{2022}', fs); draw_text(buf, bw, bh, f, "\u{2022}", x + (w - tw) / 2.0, base, fs, ink); },
        FieldKind::Submit => { let tw = f.width(&fl.label, fs); draw_text(buf, bw, bh, f, &fl.label, x + (w - tw).max(0.0) / 2.0, base, fs, ink); }
        FieldKind::Select => {
            let label = fl.options.get(fl.sel).map_or("", |o| o.1.as_str());
            let chars: Vec<char> = label.chars().collect();
            let (s, e) = visible(f, &chars, 0, fs, w - 30.0);
            draw_text(buf, bw, bh, f, &chars[s..e].iter().collect::<String>(), x + 6.0, base, fs, ink);
            let (cx, cy) = ((x + w - 14.0) as i32, (y + h / 2.0) as i32 - 2);
            for r in 0..4 { fill(buf, bw, cx - (3 - r), cy + r, 2 * (3 - r) + 1, 1, 0x444444); }
        }
        FieldKind::Text | FieldKind::Password => {
            let chars: Vec<char> = if fl.kind == FieldKind::Password { vec!['\u{2022}'; fl.value.chars().count()] } else { fl.value.chars().collect() };
            if chars.is_empty() && !fl.placeholder.is_empty() {
                let ph: Vec<char> = fl.placeholder.chars().collect();
                let (s, e) = visible(f, &ph, 0, fs, w - 12.0);
                draw_text(buf, bw, bh, f, &ph[s..e].iter().collect::<String>(), x + 6.0, base, fs, 0x8A8A8A);
            }
            let c = caret.unwrap_or(chars.len()).min(chars.len());
            let (s, e) = visible(f, &chars, c, fs, w - 12.0);
            draw_text(buf, bw, bh, f, &chars[s..e].iter().collect::<String>(), x + 6.0, base, fs, ink);
            if focused {
                let cx = x + 6.0 + f.width(&chars[s..c].iter().collect::<String>(), fs);
                fill(buf, bw, cx as i32, (y + h * 0.2) as i32, 1, (h * 0.6) as i32, ink);
            }
        }
        FieldKind::TextArea => {
            let chars: Vec<char> = fl.value.chars().collect();
            let line_h = fs * 1.35;
            let rows = (((h - 8.0) / line_h) as usize).max(1);
            let lines = wrap_lines(f, &chars, fs, w - 14.0);
            let c = caret.unwrap_or(chars.len()).min(chars.len());
            let cl = lines.iter().rposition(|&(s, _)| s <= c).unwrap_or(0);
            let first = (cl + 1).saturating_sub(rows);
            if chars.is_empty() && !fl.placeholder.is_empty() { draw_text(buf, bw, bh, f, &fl.placeholder, x + 6.0, y + 4.0 + fs, fs, 0x8A8A8A); }
            for (k, &(s, e)) in lines.iter().enumerate().skip(first).take(rows) {
                let by = y + 4.0 + fs + (k - first) as f32 * line_h;
                draw_text(buf, bw, bh, f, &chars[s..e].iter().collect::<String>(), x + 6.0, by, fs, ink);
                if focused && k == cl {
                    let cx = x + 6.0 + f.width(&chars[s..c.min(e).max(s)].iter().collect::<String>(), fs);
                    fill(buf, bw, cx as i32, (by - fs * 0.85) as i32, 1, (fs * 1.1) as i32, ink);
                }
            }
        }
        FieldKind::Hidden => {}
    }
}

/// Dropdown list under a select box at (x, y) with rows of height `h`; returns the index of the first row drawn.
pub fn draw_popup(buf: &mut [u32], bw: usize, bh: usize, f: &mut Fonts, fl: &Field, x: f32, y: f32, w: f32, h: f32) -> usize {
    let first = fl.sel.saturating_sub(POPUP_ROWS - 1).min(fl.options.len().saturating_sub(POPUP_ROWS));
    let n = fl.options.len().min(POPUP_ROWS);
    fill(buf, bw, x as i32, y as i32, w as i32, (h * n as f32) as i32 + 2, 0x767676);
    let fs = (h / 1.5).round().max(8.0);
    for (k, o) in fl.options.iter().enumerate().skip(first).take(POPUP_ROWS) {
        let ry = y + 1.0 + (k - first) as f32 * h;
        fill(buf, bw, x as i32 + 1, ry as i32, w as i32 - 2, h as i32, if k == fl.sel { 0xCFE3FC } else { 0xFFFFFF });
        let chars: Vec<char> = o.1.chars().collect();
        let (s, e) = visible(f, &chars, 0, fs, w - 12.0);
        draw_text(buf, bw, bh, f, &chars[s..e].iter().collect::<String>(), x + 6.0, ry + h * 0.5 + fs * 0.35, fs, 0x111111);
    }
    first
}

pub const POPUP_ROWS: usize = 10;

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> Option<Fonts> {
        let data = ["C:\\Windows\\Fonts\\segoeui.ttf", "C:\\Windows\\Fonts\\arial.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"].iter().find_map(|p| std::fs::read(p).ok())?;
        Some(Fonts::new(Font::from_bytes(data, fontdue::FontSettings::default()).ok()?, None))
    }

    #[test]
    fn background_pictures_tile_scale_and_offset_and_gradients_go_radial() {
        let rect = |pic| crate::layout::Rect { x: 2.0, y: 2.0, w: 8.0, h: 4.0, color: 0, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: None, pic: Some(pic), st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN };
        // a 2x1 picture: red left pixel, blue right pixel
        let im = Img { w: 2, h: 1, px: vec![255, 0, 0, 255, 0, 0, 255, 255] };
        let mut buf = vec![0xFFFFFFu32; 12 * 8];
        let tile = crate::layout::Pic { img: 0, w: 4.0, h: 4.0, ox: 0.0, oy: 0.0, rx: true, ry: false };
        draw_pic(&mut buf, 12, 8, &im, &rect(tile), 0.0);
        let px = |x: usize, y: usize| buf[y * 12 + x];
        assert_eq!((px(2, 2), px(4, 2), px(6, 2), px(8, 2)), (0xFF0000, 0x0000FF, 0xFF0000, 0x0000FF), "two tiles side by side, each a scaled 2x1 picture");
        assert_eq!((px(1, 2), px(10, 2), px(2, 7)), (0xFFFFFF, 0xFFFFFF, 0xFFFFFF), "clipped to the rectangle");
        let mut buf = vec![0xFFFFFFu32; 12 * 8];
        let once = crate::layout::Pic { img: 0, w: 4.0, h: 4.0, ox: 4.0, oy: 0.0, rx: false, ry: false };
        draw_pic(&mut buf, 12, 8, &im, &rect(once), 0.0);
        assert_eq!((buf[2 * 12 + 2], buf[2 * 12 + 6], buf[2 * 12 + 8]), (0xFFFFFF, 0xFF0000, 0x0000FF), "no-repeat at an offset draws one tile");
        // radial: the center takes the first stop, the corner the last
        let g = std::sync::Arc::new(crate::css::Grad { angle: 0.0, stops: vec![(0.0, 0xFF0000), (1.0, 0x0000FF)], radial: true, circle: true, at: (0.5, 0.5) });
        let r = crate::layout::Rect { x: 0.0, y: 0.0, w: 20.0, h: 20.0, color: 0, r: 0.0, ring: 0.0, node: 0, z: 0, a: 255, grad: Some(g), pic: None, st: f32::NAN, lim: f32::NAN, grp: 0, scroll: f32::NAN };
        let mut buf = vec![0xFFFFFFu32; 20 * 20];
        draw_rect(&mut buf, 20, 20, &r, 0.0);
        let (c, k) = (buf[10 * 20 + 10], buf[0]);
        assert!((c >> 16 & 255) > 230 && (k & 255) > 200, "center red {c:06x}, corner blue {k:06x}");
    }

    #[test]
    fn clip_restricts_every_painter_to_the_given_rectangle() {
        let mut buf = vec![0xFFFFFFu32; 20 * 10];
        set_clip(Some((5, 2, 10, 6)));
        fill(&mut buf, 20, 0, 0, 20, 10, 0xFF0000);
        draw_rrect(&mut buf, 20, 10, 0.0, 0.0, 20.0, 10.0, 0.0, 0.0, 0x0000FF);
        set_clip(None);
        let inside = |x: usize, y: usize| (5..10).contains(&x) && (2..6).contains(&y);
        for y in 0..10 { for x in 0..20 { assert_eq!(buf[y * 20 + x] != 0xFFFFFF, inside(x, y), "({x},{y})"); } }
        let mut buf = vec![0xFFFFFFu32; 60 * 20];
        let Some(mut f) = Fonts::system() else { return };
        set_clip(Some((0, 0, 20, 20)));
        draw_text(&mut buf, 60, 20, &mut f, "MMMMMMMM", 0.0, 16.0, 16.0, 0);
        set_clip(None);
        assert!((0..20).any(|y| (0..20).any(|x| buf[y * 60 + x] != 0xFFFFFF)) && (0..20).all(|y| (20..60).all(|x| buf[y * 60 + x] == 0xFFFFFF)), "text is cut at the clip edge");
    }

    #[test]
    fn wrapping_and_caret_window() {
        let Some(mut f) = fonts() else { return };
        let chars: Vec<char> = "ab\ncd".chars().collect();
        assert_eq!(wrap_lines(&mut f, &chars, 16.0, 1000.0), vec![(0, 2), (3, 5)]);
        let long: Vec<char> = "x".repeat(200).chars().collect();
        let lines = wrap_lines(&mut f, &long, 16.0, 100.0);
        assert!(lines.len() > 5 && lines.last().unwrap().1 == 200);
        let (s, e) = visible(&mut f, &long, 200, 16.0, 100.0);
        assert!(s > 0 && e == 200);
    }

    #[test]
    fn real_bold_and_italic_faces_have_their_own_metrics() {
        let Some(mut f) = Fonts::system() else { return };
        let (r, b) = (f.width_f("Hello world", 16.0, 0), f.width_f("Hello world", 16.0, BOLD));
        assert!(b > r + 0.5, "bold face is wider: {b} vs {r}");
        let m = f.width_f("Hello", 16.0, MONO);
        assert!((m - f.width_f("HHHHH", 16.0, MONO)).abs() < 0.01, "mono font has equal advances");
        assert!(f.width_f("\u{4e2d}", 16.0, 0) > 8.0, "CJK resolves through a coverage fallback");
    }

    #[test]
    fn complex_scripts_are_shaped_and_rtl_is_detected() {
        let Some(mut f) = Fonts::system() else { return };
        assert!(complex("\u{05e9}\u{05dc}\u{05d5}\u{05dd}") && complex("\u{0633}\u{0644}\u{0627}\u{0645}") && !complex("hello"));
        assert!(is_rtl("\u{05e9}\u{05dc}\u{05d5}\u{05dd}") && !is_rtl("abc \u{05e9}"));
        let arabic = "\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629}";
        let w = f.width_f(arabic, 16.0, 0);
        assert!(w > 10.0, "shaped width {w}");
        let sh = f.shape(arabic, 16.0, 0).expect("shaping works with a system font that has Arabic");
        assert!(!sh.glyphs.is_empty() && (sh.width - w).abs() < 0.01);
        let mut buf = vec![0xFFFFFFu32; 200 * 40];
        draw_text_f(&mut buf, 200, 40, &mut f, arabic, 10.0, 28.0, 20.0, 0, 0);
        assert!(buf.iter().any(|&p| p != 0xFFFFFF), "shaped glyphs are drawn");
    }
}
