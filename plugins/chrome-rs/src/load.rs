// Network + page pipeline (runs on a worker thread): fetch HTML, inline external CSS, run scripts, decode images,
// style the DOM. Returns a ready-to-layout Page; the UI thread only lays out and paints.
use crate::{dom::{self, Dom, Node}, js, render::{self, resolve, Page}, store};
use std::{collections::HashMap, sync::{mpsc, Arc}, time::{Duration, Instant}};

/// What a live page reports back to the UI: a re-rendered page after script activity, or a script-initiated navigation.
pub enum LiveEvent { Page(Page), Navigate(String) }

/// Callback for [`LiveEvent`]s (runs on a loader thread).
pub type LiveHook = Box<dyn Fn(LiveEvent) + Send + Sync>;

const UA: &str = "Mozilla/5.0 chrome-rs/0.1";
const MAX_IMG_W: u32 = 800;

/// Decoded RGBA8 image, already downscaled to at most MAX_IMG_W wide.
#[derive(Clone)]
pub struct Img { pub w: u32, pub h: u32, pub px: Vec<u8> }

/// Network archive for the real-page corpus: while recording (`true`) every GET answer is stored under the directory
/// (`index.tsv` + one `.bin` per response); while replaying (`false`) answers come from there and the network is never touched.
static ARCHIVE: std::sync::Mutex<Option<(std::path::PathBuf, bool)>> = std::sync::Mutex::new(None);
static ARCHIVE_INDEX: std::sync::Mutex<Option<HashMap<String, Vec<String>>>> = std::sync::Mutex::new(None);

pub fn set_archive(dir: Option<(std::path::PathBuf, bool)>) {
    let index = dir.as_ref().filter(|d| !d.1).map(|d| {
        std::fs::read_to_string(d.0.join("index.tsv")).unwrap_or_default().lines()
            .filter_map(|l| { let f: Vec<String> = l.split('\t').map(String::from).collect(); (f.len() == 7).then(|| (f[0].clone(), f[1..].to_vec())) }).collect()
    });
    if let Ok(mut i) = ARCHIVE_INDEX.lock() { *i = index; }
    if let Ok(mut a) = ARCHIVE.lock() { *a = dir; }
}

/// True while pages are served from a network archive (the corpus), when scripts must be deterministic.
pub fn replaying() -> bool { ARCHIVE.lock().is_ok_and(|a| a.as_ref().is_some_and(|d| !d.1)) }

fn archive_file(key: &str) -> String { format!("{:016x}.bin", key.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))) }

fn archived(method: &str, url: &str) -> Option<Result<Resp, String>> {
    let a = ARCHIVE.lock().ok()?;
    let (dir, record) = a.as_ref()?;
    if *record { return None; }
    let key = format!("{} {url}", method.to_ascii_uppercase());
    let idx = ARCHIVE_INDEX.lock().ok()?;
    let Some(f) = idx.as_ref()?.get(&key) else { return Some(Err("not in the archive".into())) };
    let opt = |s: &String| (!s.is_empty()).then(|| s.clone());
    Some(std::fs::read(dir.join(&f[0])).map_err(|e| e.to_string()).map(|bytes| Resp { url: f[1].clone(), status: f[2].parse().unwrap_or(200), mime: f[3].clone(), charset: opt(&f[4]), bytes, acao: opt(&f[5]) }))
}

fn record(method: &str, url: &str, r: &Resp) {
    let Ok(a) = ARCHIVE.lock() else { return };
    let Some((dir, true)) = a.as_ref() else { return };
    let key = format!("{} {url}", method.to_ascii_uppercase());
    let file = archive_file(&key);
    if std::fs::write(dir.join(&file), &r.bytes).is_err() { return; }
    use std::io::Write;
    if let Ok(mut idx) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("index.tsv")) {
        let _ = writeln!(idx, "{key}\t{file}\t{}\t{}\t{}\t{}\t{}", r.url, r.status, r.mime, r.charset.as_deref().unwrap_or(""), r.acao.as_deref().unwrap_or(""));
    }
}

/// In-memory HTTP cache for GET responses that arrived without redirects: fresh by `max-age`/`Expires`, otherwise revalidated with ETag / Last-Modified.
/// shortcut: memory only (64 MB cap, wiped when full), no `Vary`, no heuristic freshness; persist to disk if cold starts matter.
struct Cached { resp: Resp, fresh_until: std::time::Instant, etag: Option<String>, modified: Option<String>, size: usize }
static CACHE: std::sync::LazyLock<std::sync::Mutex<(HashMap<String, Cached>, usize)>> = std::sync::LazyLock::new(Default::default);
const CACHE_CAP: usize = 64 << 20;

fn max_age(cc: &str) -> Option<u64> {
    cc.split(',').map(str::trim).find_map(|d| d.strip_prefix("max-age=").or(d.strip_prefix("s-maxage=")).and_then(|v| v.trim_matches('"').parse().ok()))
}

fn cache_store(url: &str, r: &Resp, cc: &str, etag: Option<String>, modified: Option<String>) {
    let lc = cc.to_ascii_lowercase();
    if r.status != 200 || lc.contains("no-store") { return; }
    let age = if lc.contains("no-cache") { 0 } else { max_age(&lc).unwrap_or(0) };
    if age == 0 && etag.is_none() && modified.is_none() { return; }
    let size = r.bytes.len();
    if size > CACHE_CAP / 4 { return; }
    let Ok(mut c) = CACHE.lock() else { return };
    if c.1 + size > CACHE_CAP { c.0.clear(); c.1 = 0; }
    c.1 += size;
    c.0.insert(url.to_string(), Cached { resp: r.clone(), fresh_until: std::time::Instant::now() + Duration::from_secs(age), etag, modified, size });
}

fn agent(secs: u64) -> ureq::Agent {
    ureq::Agent::new_with_config(ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(secs))).http_status_as_error(false).max_redirects(0).build())
}

#[derive(Clone)]
struct Resp { url: String, status: u16, mime: String, charset: Option<String>, bytes: Vec<u8>, acao: Option<String> }

/// `page`: host of the page that triggered a subresource request; cookies are only sent/stored for the same site. None = top-level navigation.
fn get(agent: &ureq::Agent, url: &str, limit: u64, page: Option<&str>) -> Result<Resp, String> { request(agent, url, None, limit, page) }

/// GET, or a urlencoded POST when `post` carries the body. Redirects are followed by hand so every hop sees and sets cookies.
fn request(agent: &ureq::Agent, url: &str, post: Option<&str>, limit: u64, page: Option<&str>) -> Result<Resp, String> {
    match post {
        Some(body) => request_with(agent, url, "POST", Some((body, "application/x-www-form-urlencoded")), limit, page),
        None => request_with(agent, url, "GET", None, limit, page),
    }
}

/// Any HTTP method; `body` = (text, content-type). 301-303 redirects turn into GET, 307/308 keep method and body.
fn request_with(agent: &ureq::Agent, url: &str, method: &str, body: Option<(&str, &str)>, limit: u64, page: Option<&str>) -> Result<Resp, String> {
    if let Some(r) = local(url, limit, page) { return r; }
    if let Some(r) = archived(method, url) { return r; }
    let first_url = url.to_string();
    let mut validators: Option<(Option<String>, Option<String>)> = None;
    if method.eq_ignore_ascii_case("GET") {
        if let Ok(c) = CACHE.lock() {
            if let Some(e) = c.0.get(&first_url) {
                if e.fresh_until > std::time::Instant::now() { return Ok(e.resp.clone()); }
                validators = Some((e.etag.clone(), e.modified.clone()));
            }
        }
    }
    let (mut url, mut method, mut body) = (url.to_string(), method.to_ascii_uppercase(), body);
    let asked = method.clone();
    for _ in 0..10 {
        let host = store::split_url(&url).map_or("", |u| u.1).to_string();
        let jar = page.is_none_or(|p| store::same_site(p, &host));
        let ck = if jar { store::cookie_header(&url, true) } else { String::new() };
        let sent = match (method.as_str(), body) {
            ("POST" | "PUT" | "PATCH", b) => {
                let (text, ctype) = b.unwrap_or(("", ""));
                let mut r = match method.as_str() { "POST" => agent.post(&url), "PUT" => agent.put(&url), _ => agent.patch(&url) }.header("User-Agent", UA);
                if !ctype.is_empty() { r = r.header("Content-Type", ctype); }
                if !ck.is_empty() { r = r.header("Cookie", &ck); }
                r.send(text)
            }
            (m, _) => {
                let mut r = match m { "DELETE" => agent.delete(&url), "HEAD" => agent.head(&url), _ => agent.get(&url) }.header("User-Agent", UA);
                if let (true, Some((etag, modified))) = (url == first_url, &validators) {
                    if let Some(e) = etag { r = r.header("If-None-Match", e); }
                    if let Some(m) = modified { r = r.header("If-Modified-Since", m); }
                }
                if !ck.is_empty() { r = r.header("Cookie", &ck); }
                r.call()
            }
        };
        let mut resp = sent.map_err(|e| e.to_string())?;
        if jar { for h in resp.headers().get_all("set-cookie") { if let Ok(h) = h.to_str() { store::set_cookie(&url, h, false); } } }
        let status = resp.status().as_u16();
        if status == 304 && url == first_url {
            if let Ok(mut c) = CACHE.lock() {
                if let Some(e) = c.0.get_mut(&first_url) {
                    let age = resp.headers().get("cache-control").and_then(|v| v.to_str().ok()).and_then(max_age).unwrap_or(0);
                    e.fresh_until = std::time::Instant::now() + Duration::from_secs(age);
                    return Ok(e.resp.clone());
                }
            }
        }
        let hdr = |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).map(String::from);
        let (cc, etag, modified) = (hdr("cache-control").unwrap_or_default(), hdr("etag"), hdr("last-modified"));
        let loc = resp.headers().get("location").and_then(|l| l.to_str().ok()).map(|l| resolve(&url, l));
        if let (300..400, Some(next)) = (status, loc) {
            if !matches!(status, 307 | 308) { body = None; method = "GET".into(); }
            url = next;
            continue;
        }
        let resp_acao = resp.headers().get("access-control-allow-origin").and_then(|v| v.to_str().ok()).map(String::from);
        let mime = resp.body().mime_type().unwrap_or("text/html").to_string();
        let charset = resp.body().charset().map(String::from);
        let bytes = resp.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| e.to_string())?;
        let acao = resp_acao;
        let r = Resp { url, status, mime, charset, bytes, acao };
        if asked == "GET" {
            record(&asked, &first_url, &r);
            if r.url == first_url { cache_store(&first_url, &r, &cc, etag, modified); }
        }
        return Ok(r);
    }
    Err("too many redirects".into())
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let (mut out, mut i) = (Vec::with_capacity(b.len()), 0);
    while i < b.len() {
        match (b[i], b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => { out.push(v); i += 3; }
            (c, _) => { out.push(c); i += 1; }
        }
    }
    out
}

fn base64_decode(s: &str) -> Vec<u8> {
    let (mut out, mut acc, mut bits) = (vec![], 0u32, 0);
    for c in s.bytes() {
        let v = match c { b'A'..=b'Z' => c - b'A', b'a'..=b'z' => c - b'a' + 26, b'0'..=b'9' => c - b'0' + 52, b'+' | b'-' => 62, b'/' | b'_' => 63, _ => continue };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 { bits -= 8; out.push((acc >> bits) as u8); acc &= (1 << bits) - 1; }
    }
    out
}

fn escape_html(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }

fn mime_of_ext(path: &str) -> &'static str {
    match path.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("html" | "htm" | "xhtml") => "text/html",
        Some("css") => "text/css",
        Some("js" | "mjs") => "text/javascript",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "text/plain",
    }
}

/// Non-network schemes: `file:`, `data:`, `about:` and `view-source:`. None for everything else.
fn local(url: &str, limit: u64, page: Option<&str>) -> Option<Result<Resp, String>> {
    let resp = |mime: &str, bytes: Vec<u8>| Ok(Resp { url: url.to_string(), status: 200, mime: mime.to_string(), charset: None, bytes, acao: None });
    if let Some(rest) = url.strip_prefix("file://") {
        // a remote page may not read local files; only top-level navigation and file: pages themselves may
        if page.is_some_and(|p| !p.is_empty()) { return Some(Err("blocked: file: from a remote page".into())); } // file pages have an empty host
        let path = String::from_utf8_lossy(&percent_decode(rest.split(['?', '#']).next().unwrap_or(rest))).into_owned();
        let path = if path.len() > 2 && path.as_bytes()[0] == b'/' && path.as_bytes()[2] == b':' { path[1..].to_string() } else { path };
        return Some(std::fs::metadata(&path).map_err(|e| format!("{path}: {e}")).and_then(|m| if m.len() > limit { Err(format!("{path}: file too large")) } else { std::fs::read(&path).map_err(|e| e.to_string()) })
            .and_then(|bytes| resp(mime_of_ext(&path), bytes)));
    }
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, payload) = rest.split_once(',')?;
        let b64 = meta.ends_with(";base64");
        let mime = meta.split(';').next().filter(|m| !m.is_empty()).unwrap_or("text/plain");
        let bytes = if b64 { base64_decode(&percent_decode(payload).iter().map(|&c| c as char).collect::<String>()) } else { percent_decode(payload) };
        return Some(if bytes.len() as u64 > limit { Err("data: URL too large".into()) } else { resp(mime, bytes) });
    }
    if url.starts_with("about:") { return Some(resp("text/html", store::about_html(url).unwrap_or_else(|| "<title>about:blank</title>".into()).into_bytes())); }
    if let Some(inner) = url.strip_prefix("view-source:") {
        return Some(request(&agent(20), inner, None, limit, None).map(|r| {
            let body = format!("<title>view-source:{}</title><pre>{}</pre>", escape_html(inner), escape_html(&decode_text(&r)));
            Resp { url: url.to_string(), status: r.status, mime: "text/html".into(), charset: None, bytes: body.into_bytes(), acao: None }
        }));
    }
    None
}

/// WOFF 1.0 / WOFF 2.0 -> plain sfnt bytes (WOFF1 tables are inflated, WOFF2 goes through brotli and the glyf/loca transform); sfnt formats pass through.
fn decode_font(b: &[u8]) -> Option<Vec<u8>> {
    match b.get(..4)? {
        b"wOFF" => {
            let be16 = |o: usize| b.get(o..o + 2).map(|x| u16::from_be_bytes([x[0], x[1]]) as usize);
            let be32 = |o: usize| b.get(o..o + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]) as usize);
            let n = be16(12)?;
            if n == 0 || n > 64 { return None; }
            let mut tables = vec![];
            for i in 0..n {
                let e = 44 + i * 20;
                let (off, comp, orig) = (be32(e + 4)?, be32(e + 8)?, be32(e + 12)?);
                let raw = b.get(off..off.checked_add(comp)?)?;
                let data = if comp < orig { miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(raw, 8 << 20).ok().filter(|d| d.len() == orig)? } else { raw.to_vec() };
                tables.push((b[e..e + 4].to_vec(), b[e + 16..e + 20].to_vec(), data));
            }
            let mut out = Vec::with_capacity(be32(16)?.min(16 << 20));
            out.extend_from_slice(&b[4..8]); // flavor
            out.extend_from_slice(&(n as u16).to_be_bytes());
            let mut sr = 1usize;
            let mut es = 0u16;
            while sr * 2 <= n { sr *= 2; es += 1; }
            out.extend_from_slice(&((sr * 16) as u16).to_be_bytes());
            out.extend_from_slice(&es.to_be_bytes());
            out.extend_from_slice(&((n * 16 - sr * 16) as u16).to_be_bytes());
            let mut off = 12 + n * 16;
            for (tag, sum, data) in &tables {
                out.extend_from_slice(tag);
                out.extend_from_slice(sum);
                out.extend_from_slice(&(off as u32).to_be_bytes());
                out.extend_from_slice(&(data.len() as u32).to_be_bytes());
                off += data.len().div_ceil(4) * 4;
            }
            for (_, _, data) in &tables {
                out.extend_from_slice(data);
                out.resize(out.len().div_ceil(4) * 4, 0);
            }
            Some(out)
        }
        b"wOF2" => woff2_patched::decode::convert_woff2_to_ttf(&mut &b[..]).ok().filter(|d| d.len() <= 16 << 20),
        b"\0\x01\0\0" | b"OTTO" | b"true" | b"ttcf" => Some(b.to_vec()),
        _ => None,
    }
}

/// Rewrites relative `url(...)` references of a stylesheet to absolute ones (the sheet is inlined, so its own address is lost).
fn absolutize_css_urls(css: &str, sheet_url: &str) -> String {
    let (mut out, mut rest) = (String::with_capacity(css.len()), css);
    while let Some(p) = rest.find("url(") {
        out.push_str(&rest[..p + 4]);
        let after = &rest[p + 4..];
        let Some(end) = after.find(')') else { rest = after; break };
        let raw = after[..end].trim();
        let url = raw.trim_matches(['"', '\'']);
        if url.is_empty() || url.starts_with("data:") || url.starts_with('#') { out.push_str(&after[..end]); } else { out.push_str(&format!("\"{}\"", resolve(sheet_url, url))); }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Downloads the first usable source of each `@font-face` rule (WOFF2, WOFF, TTF or OTF).
fn fetch_web_fonts(faces: &[crate::css::FontFaceRule], base: &str, page: &str, local_ok: bool) -> Vec<crate::render::WebFont> {
    let pick = |f: &crate::css::FontFaceRule| f.srcs.iter().map(|(u, fmt)| (resolve(base, u), fmt)).find(|(u, fmt)| {
        let u = u.to_ascii_lowercase();
        let path = u.split(['?', '#']).next().unwrap_or("");
        let ok_kind = matches!(fmt.as_str(), "woff2" | "woff" | "truetype" | "opentype" | "ttf" | "otf") || (fmt.is_empty() && [".woff2", ".woff", ".ttf", ".otf"].iter().any(|e| path.ends_with(e)));
        ok_kind && (u.starts_with("http") || local_ok && u.starts_with("file:"))
    }).map(|s| s.0);
    let list: Vec<(usize, String)> = faces.iter().enumerate().filter_map(|(i, f)| pick(f).map(|u| (i, u))).take(8).collect();
    let data = fetch_all(&list, 3_000_000, page, |r| decode_font(&r.bytes));
    list.iter().zip(data).filter_map(|((i, _), d)| Some(crate::render::WebFont { fam: crate::gfx::family_id(&faces[*i].family), bold: faces[*i].bold, italic: faces[*i].italic, data: d? })).collect()
}

fn sniff_meta_charset(b: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&b[..b.len().min(2048)]).to_ascii_lowercase();
    let rest = head[head.find("charset=")? + 8..].trim_start_matches(['"', '\'']);
    Some(rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect())
}

fn decode_text(r: &Resp) -> String {
    let enc = r.charset.clone().or_else(|| sniff_meta_charset(&r.bytes)).and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()));
    match enc {
        Some(e) if e != encoding_rs::UTF_8 => e.decode(&r.bytes).0.into_owned(),
        _ => String::from_utf8_lossy(&r.bytes).into_owned(),
    }
}

/// Synchronous request for page scripts (fetch/XHR): (status, body, content-type); None on network failure, non-text or a CORS block.
/// Cross-origin requests follow a simplified CORS: only GET/HEAD/POST with a simple content type are sent (no preflight support),
/// and the script only sees the response when `Access-Control-Allow-Origin` is `*` or the page's origin.
pub fn fetch_req(url: &str, page_url: &str, method: &str, body: &str, ctype: &str) -> Option<(u16, String, String)> {
    let page_origin = store::origin_of(page_url);
    let cross = store::origin_of(url) != page_origin;
    let simple_type = ctype.is_empty() || ["application/x-www-form-urlencoded", "multipart/form-data", "text/plain"].iter().any(|t| ctype.to_ascii_lowercase().starts_with(t));
    if cross && !(matches!(method, "GET" | "HEAD") || method == "POST" && simple_type) { return None; }
    let host = store::split_url(page_url).map_or("", |u| u.1);
    let r = request_with(&agent(6), url, method, Some((body, ctype)), 1_000_000, Some(host)).ok()?;
    if cross && !r.acao.as_deref().is_some_and(|a| a == "*" || a == page_origin) { return None; }
    Some((r.status, decode_text(&r), r.mime.clone()))
}

fn decode_image(bytes: &[u8]) -> Option<Img> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
    if head.contains("<svg") { return crate::svg::render(&String::from_utf8_lossy(bytes)); }
    let im = image::load_from_memory(bytes).ok()?;
    let im = if im.width() > MAX_IMG_W { im.thumbnail(MAX_IMG_W, 4000) } else { im };
    let rgba = im.to_rgba8();
    Some(Img { w: rgba.width(), h: rgba.height(), px: rgba.into_raw() })
}

fn is_module(dom: &Dom, id: usize) -> bool { dom.nodes[id].tag == "script" && dom.attr(id, "type").is_some_and(|t| t.eq_ignore_ascii_case("module")) }

fn is_js(dom: &Dom, id: usize) -> bool {
    dom.nodes[id].tag == "script" && dom.attr(id, "type").is_none_or(|t| { let t = t.to_ascii_lowercase(); t.is_empty() || t.contains("javascript") || t.contains("ecmascript") })
}

/// Fetches all `list` URLs in parallel, mapping each response body with `f`.
fn fetch_all<T: Send>(list: &[(usize, String)], limit: u64, page: &str, f: impl Fn(Resp) -> Option<T> + Sync) -> Vec<Option<T>> {
    let ag = agent(8);
    std::thread::scope(|s| {
        let hs: Vec<_> = list.iter().map(|(_, u)| { let (ag, f) = (&ag, &f); s.spawn(move || get(ag, u, limit, Some(page)).ok().filter(|r| r.status < 400).and_then(f)) }).collect();
        hs.into_iter().map(|h| h.join().ok().flatten()).collect()
    })
}

/// Offline page for regression snapshots: inline CSS and scripts only, no network, no images.
pub fn load_offline(html: &str, url: &str) -> Page {
    let mut dom = dom::parse(html, true);
    let scripts: Vec<js::Script> = dom.descendants().into_iter().filter(|&i| is_js(&dom, i) && dom.attr(i, "src").is_none()).map(|i| js::Script::classic(dom.text_content(i))).collect();
    js::run(&mut dom, &scripts, url);
    render::build(&dom, url, HashMap::new())
}

pub fn load(url: &str, post: Option<&str>, hook: Option<LiveHook>) -> Result<(String, Page), String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load_inner(url, post, hook))).unwrap_or_else(|_| Err("internal error while loading this page".into()))
}

/// Decoded images by URL (None = failed once, not retried) so re-rendering a live page does not refetch.
type ImgCache = HashMap<String, Option<Img>>;

/// Candidate of a `srcset`: (url, descriptor value, is a width descriptor).
/// Candidate of a `srcset`: (url, descriptor value, is a width descriptor).
fn parse_srcset(s: &str) -> Vec<(String, f32, bool)> {
    let mut out = vec![];
    let mut rest = s.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    while !rest.is_empty() {
        // the url runs to the first whitespace (it may contain commas); a trailing comma ends a candidate without descriptor
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (url, after) = rest.split_at(end);
        let bare = url.ends_with(',');
        let url = url.trim_end_matches(',');
        let (desc, tail) = if bare { ("", after) } else { match after.find(',') { Some(c) => (&after[..c], &after[c + 1..]), None => (after, "") } };
        let desc = desc.trim();
        let (v, is_w) = if let Some(w) = desc.strip_suffix('w') { (w.parse().unwrap_or(0.0), true) } else if let Some(x) = desc.strip_suffix('x') { (x.parse().unwrap_or(1.0), false) } else { (1.0, false) };
        if !url.is_empty() { out.push((url.to_string(), v, is_w)); }
        rest = tail.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    }
    out
}

/// The candidate a 1000 px wide screen at 1x would use: the smallest width descriptor of at least 800, else the largest; for density descriptors the 1x one.
fn pick_srcset(s: &str) -> Option<String> {
    let c = parse_srcset(s);
    if c.is_empty() { return None; }
    if c.iter().any(|x| x.2) {
        let mut w: Vec<_> = c.iter().filter(|x| x.2).collect();
        w.sort_by(|a, b| a.1.total_cmp(&b.1));
        return w.iter().find(|x| x.1 >= 800.0).or(w.last()).map(|x| x.0.clone());
    }
    c.iter().min_by(|a, b| (a.1 - 1.0).abs().total_cmp(&(b.1 - 1.0).abs())).map(|x| x.0.clone())
}

/// Does a `<source media>` query hold on a 1000 px wide screen? Only min-width/max-width in px are understood; anything else counts as a match.
fn media_matches(m: &str) -> bool {
    let m = m.to_ascii_lowercase();
    let px = |key: &str| m.split(key).nth(1).and_then(|r| r.trim_start_matches([':', ' ']).split(|c: char| !c.is_ascii_digit() && c != '.').next()).and_then(|v| v.parse::<f32>().ok());
    px("min-width").is_none_or(|v| 1000.0 >= v) && px("max-width").is_none_or(|v| 1000.0 <= v)
}

/// URL of the picture an `<img>` shows: a matching `<picture><source>`, then `srcset`, then `src`, then lazy-loading attributes (`data-src`, `data-srcset`).
fn img_url(dom: &Dom, i: usize) -> Option<String> {
    let usable = |t: &str| { let t = t.trim().to_ascii_lowercase(); t.is_empty() || matches!(t.as_str(), "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp" | "image/svg+xml") };
    let parent = dom.nodes[i].parent;
    if parent != 0 && dom.nodes[parent].tag == "picture" {
        for &k in &dom.nodes[parent].kids {
            if k == i { break; }
            if dom.nodes[k].tag != "source" || !usable(dom.attr(k, "type").unwrap_or_default()) || !dom.attr(k, "media").is_none_or(|m| media_matches(&m)) { continue; }
            if let Some(u) = dom.attr(k, "srcset").or_else(|| dom.attr(k, "data-srcset")).and_then(pick_srcset) { return Some(u); }
        }
    }
    if let Some(u) = dom.attr(i, "srcset").or_else(|| dom.attr(i, "data-srcset")).and_then(pick_srcset) { return Some(u); }
    dom.attr(i, "src").filter(|s| !s.is_empty() && !s.starts_with("data:")).or_else(|| dom.attr(i, "data-src")).map(String::from)
}

/// Images of `dom` keyed by node id: `<img>` sources (downloaded once per URL) and outermost inline `<svg>` (rasterized once per markup).
fn collect_images(dom: &Dom, base: &str, page: &str, local_ok: bool, cache: &mut ImgCache) -> HashMap<usize, Img> {
    let img_src: Vec<(usize, String)> = dom.descendants().into_iter().filter(|&i| dom.nodes[i].tag == "img").filter_map(|i| {
        let s = img_url(dom, i)?;
        Some((i, resolve(base, &s)))
    }).filter(|(_, u)| u.starts_with("http") || local_ok && u.starts_with("file:")).take(24).collect();
    // CSS background images are keyed `usize::MAX - element id` so they never collide with `<img>` nodes
    let bg: Vec<(usize, String)> = render::background_urls(dom, base).into_iter().filter(|(_, u)| u.starts_with("http") || u.starts_with("data:") || local_ok && u.starts_with("file:")).map(|(i, u)| (usize::MAX - i, u)).collect();
    let img_src: Vec<(usize, String)> = img_src.into_iter().chain(bg).collect();
    let mut missing: Vec<(usize, String)> = vec![];
    for (i, u) in &img_src { if !cache.contains_key(u) && !missing.iter().any(|(_, m)| m == u) { missing.push((*i, u.clone())); } }
    let decoded = fetch_all(&missing, 4_000_000, page, |r| decode_image(&r.bytes));
    for ((_, u), im) in missing.into_iter().zip(decoded) { cache.insert(u, im); }
    let mut imgs: HashMap<usize, Img> = img_src.iter().filter_map(|(i, u)| Some((*i, cache.get(u)?.clone()?))).collect();
    let inline: Vec<usize> = dom.descendants().into_iter().filter(|&i| dom.nodes[i].tag == "svg").filter(|&i| {
        let mut p = dom.nodes[i].parent;
        while p != 0 { if dom.nodes[p].tag == "svg" { return false; } p = dom.nodes[p].parent; }
        true
    }).take(40).collect();
    for i in inline {
        let markup = crate::svg::to_markup(dom, i);
        let key = format!("svg:{markup}");
        if !cache.contains_key(&key) { let im = crate::svg::render(&markup); cache.insert(key.clone(), im); }
        if let Some(Some(im)) = cache.get(&key) { imgs.insert(i, im.clone()); }
    }
    imgs
}

/// Thread that turns each serialized DOM from a live script session into a Page (latest wins, at most ~12 per second).
fn spawn_builder(rx: mpsc::Receiver<String>, base: String, host: String, mut cache: ImgCache, hook: Arc<LiveHook>) {
    let local = base.starts_with("file:");
    let _ = std::thread::Builder::new().stack_size(32 << 20).spawn(move || {
        while let Ok(mut json) = rx.recv() {
            while let Ok(j) = rx.try_recv() { json = j; }
            let Some(dom) = serde_json::from_str(&json).ok().and_then(|v| Dom::from_json(&v)) else { continue };
            let page = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| { let imgs = collect_images(&dom, &base, &host, local, &mut cache); render::build(&dom, &base, imgs) }));
            if let Ok(p) = page { hook(LiveEvent::Page(p)); }
            std::thread::sleep(Duration::from_millis(80));
        }
    });
}

fn load_inner(url: &str, post: Option<&str>, hook: Option<LiveHook>) -> Result<(String, Page), String> {
    let timing = std::env::var_os("CHROME_RS_TIMING").is_some();
    let t0 = Instant::now();
    let lap = |what: &str| if timing { eprintln!("{:>6} ms  {what}", t0.elapsed().as_millis()); };

    let main = request(&agent(20), url, post, 12_000_000, None)?;
    if !(main.mime.starts_with("text/") || main.mime.contains("xml")) { return Err(format!("unsupported content type: {}", main.mime)); }
    let final_url = main.url.clone();
    let local_ok = final_url.starts_with("file:");
    let fetchable = |u: &str| u.starts_with("http") || local_ok && u.starts_with("file:");
    let page_host = store::split_url(&final_url).map_or(String::new(), |u| u.1.to_string());
    let text = decode_text(&main);
    let mut dom = dom::parse(&if main.mime == "text/plain" { format!("<pre>{}</pre>", escape_html(&text)) } else { text }, true);
    lap("html fetched + parsed");
    let ids = dom.descendants();

    let css: Vec<(usize, String)> = ids.iter().filter(|&&i| {
        dom.nodes[i].tag == "link" && dom.attr(i, "rel").is_some_and(|r| r.to_ascii_lowercase().contains("stylesheet")) && !dom.attr(i, "media").is_some_and(|m| m.contains("print"))
    }).filter_map(|&i| dom.attr(i, "href").map(|h| (i, resolve(&final_url, h)))).filter(|(_, u)| fetchable(u)).take(12).collect();
    let js_src: Vec<(usize, String)> = ids.iter().filter(|&&i| is_js(&dom, i) || is_module(&dom, i)).filter_map(|&i| dom.attr(i, "src").map(|s| (i, resolve(&final_url, s)))).filter(|(_, u)| fetchable(u)).take(16).collect();

    // CSS and JS downloads overlap with each other.
    let (css_text, js_text) = std::thread::scope(|s| {
        let c = s.spawn(|| fetch_all(&css, 2_000_000, &page_host, |r| Some(decode_text(&r))));
        let j = fetch_all(&js_src, 2_000_000, &page_host, |r| Some(decode_text(&r)));
        (c.join().unwrap_or_default(), j)
    });
    lap("css + js downloaded");

    for ((id, _), text) in css.iter().zip(css_text) {
        if let Some(t) = text {
            dom.nodes[*id].tag = "style".into();
            dom.nodes[*id].attrs.clear();
            dom.add(*id, Node { text: absolutize_css_urls(&t, &css[css.iter().position(|(i, _)| i == id).unwrap_or(0)].1), ..Default::default() });
        }
    }
    // classic scripts in document order, then modules (they are deferred), each with the URL that anchors its imports
    let (mut scripts, mut modules, mut inline) = (vec![], vec![], 0);
    for &i in &ids {
        if dom.nodes[i].tag == "script" && dom.attr(i, "type").is_some_and(|t| t.eq_ignore_ascii_case("importmap")) { scripts.push(js::Script::importmap(dom.text_content(i))); continue; }
        let module = is_module(&dom, i);
        if !(module || is_js(&dom, i)) { continue; }
        let (code, name) = if dom.attr(i, "src").is_some() {
            let Some(p) = js_src.iter().position(|(j, _)| *j == i) else { continue };
            let Some(t) = js_text[p].clone() else { continue };
            (t, js_src[p].1.clone())
        } else { inline += 1; (dom.text_content(i), format!("{final_url}#inline-{inline}")) };
        (if module { &mut modules } else { &mut scripts }).push(js::Script { module, name, code });
    }
    scripts.extend(modules);
    // With a hook the scripts stay alive after load (timers, events, DOM updates); otherwise they run once.
    // A page with no scripts and no inline `on...=` handlers needs no JavaScript runtime at all (saves a thread and ~25 MB).
    let needs_js = !scripts.is_empty() || dom.nodes.iter().any(|n| n.attrs.iter().any(|(k, _)| k.starts_with("on")));
    let hook = hook.map(Arc::new);
    let (utx, urx) = mpsc::channel::<String>();
    let mut live = None;
    if let (true, Some(h), Some(&html)) = (needs_js, &hook, dom.nodes[0].kids.first()) {
        let hn = h.clone();
        let nav: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |u| hn(LiveEvent::Navigate(u)));
        if let Some((handle, first)) = js::start(dom.to_json(html).to_string(), scripts.clone(), final_url.clone(), Box::new(move |s| { let _ = utx.send(s); }), nav) {
            if let Some(d) = serde_json::from_str(&first).ok().and_then(|v| Dom::from_json(&v)) { dom = d; live = Some(handle); }
        }
    }
    if live.is_none() && needs_js { js::run(&mut dom, &scripts, &final_url); }
    lap("scripts executed");

    let mut cache = ImgCache::new();
    let imgs = collect_images(&dom, &final_url, &page_host, local_ok, &mut cache);
    lap("images decoded");

    let mut page = render::build(&dom, &final_url, imgs);
    lap("styled");
    page.webfonts = fetch_web_fonts(&page.faces, &final_url, &page_host, final_url.starts_with("file:"));
    lap("web fonts");
    if let (Some(handle), Some(h)) = (live, hook) {
        page.live = Some(handle);
        spawn_builder(urx, final_url.clone(), page_host, cache, h);
    }
    Ok((final_url, page))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(r: Option<Result<Resp, String>>) -> (String, String) { let r = r.unwrap().unwrap(); (r.mime, String::from_utf8(r.bytes).unwrap()) }

    #[test]
    fn data_and_about_urls() {
        assert_eq!(body(local("data:text/html,%3Cb%3Ehi%3C/b%3E", 1000, None)), ("text/html".into(), "<b>hi</b>".into()));
        assert_eq!(body(local("data:text/plain;base64,aGVsbG8gd29ybGQ=", 1000, None)), ("text/plain".into(), "hello world".into()));
        assert!(body(local("about:blank", 10, None)).1.contains("about:blank"));
        assert!(local("https://a.com", 10, None).is_none());
        assert!(local("data:text/plain,toolongpayload", 4, None).unwrap().is_err());
    }

    #[test]
    fn file_urls_open_locally_but_never_from_remote_pages() {
        let path = std::env::temp_dir().join(format!("chrome-rs-file-{}.html", std::process::id()));
        std::fs::write(&path, "<p>local</p>").unwrap();
        let url = format!("file:///{}", path.to_string_lossy().replace('\\', "/").replace(' ', "%20"));
        assert_eq!(body(local(&url, 1000, None)), ("text/html".into(), "<p>local</p>".into()));
        assert_eq!(body(local(&url, 1000, Some(""))).1, "<p>local</p>"); // a file: page may load siblings
        assert!(local(&url, 1000, Some("evil.com")).unwrap().is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn plain_text_helpers() {
        assert_eq!(escape_html("<a&b>"), "&lt;a&amp;b&gt;");
        assert_eq!(mime_of_ext("C:/x/readme.MD"), "text/plain");
        assert_eq!(mime_of_ext("a.HTM"), "text/html");
    }

    #[test]
    fn cookies_redirects_and_methods_over_http() {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..4 { // login, home (after redirect), echo, home
                let Ok((mut c, _)) = l.accept() else { break };
                let mut buf = vec![];
                let mut chunk = [0u8; 1024];
                let (head_end, clen) = loop {
                    let n = c.read(&mut chunk).unwrap_or(0);
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(p) = text.find("\r\n\r\n") {
                        let cl = text.to_ascii_lowercase().lines().find_map(|l| l.strip_prefix("content-length: ").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
                        if buf.len() >= p + 4 + cl || n == 0 { break (p + 4, cl); }
                    } else if n == 0 { break (buf.len(), 0); }
                };
                let text = String::from_utf8_lossy(&buf).to_string();
                let (head, body) = (text[..head_end.min(text.len())].to_string(), text[head_end.min(text.len())..].chars().take(clen).collect::<String>());
                let first = head.lines().next().unwrap_or("").to_string();
                let (resp_head, resp_body) = if first.contains("/login") {
                    ("HTTP/1.1 302 Found\r\nLocation: /home\r\nSet-Cookie: sid=abc; Path=/; HttpOnly".to_string(), String::new())
                } else if first.contains("/home") {
                    ("HTTP/1.1 200 OK\r\nContent-Type: text/html".to_string(), if head.to_ascii_lowercase().contains("cookie: sid=abc") { "welcome".into() } else { "anon".into() })
                } else {
                    let ct = head.to_ascii_lowercase().lines().find_map(|l| l.strip_prefix("content-type: ").map(String::from)).unwrap_or_default();
                    ("HTTP/1.1 200 OK\r\nContent-Type: text/plain".to_string(), format!("{} {} {}", first.split(' ').next().unwrap_or(""), ct, body))
                };
                let _ = c.write_all(format!("{resp_head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}", resp_body.len()).as_bytes());
            }
        });
        let base = format!("http://127.0.0.1:{port}");
        let ag = agent(5);
        let r = request_with(&ag, &format!("{base}/login"), "GET", None, 10_000, None).unwrap();
        assert_eq!((r.status, r.url.as_str(), String::from_utf8_lossy(&r.bytes).as_ref()), (200, format!("{base}/home").as_str(), "welcome")); // cookie set on the 302 hop, sent on the next
        let r = request_with(&ag, &format!("{base}/echo"), "PUT", Some(("x=1", "application/json")), 10_000, None).unwrap();
        assert_eq!(String::from_utf8_lossy(&r.bytes), "PUT application/json x=1");
        let r = request_with(&ag, &format!("{base}/home"), "GET", None, 10_000, Some("other.com")).unwrap();
        assert_eq!(String::from_utf8_lossy(&r.bytes), "anon"); // third-party request gets no cookies
    }

    #[test]
    fn cross_origin_requests_need_cors_approval() {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        std::thread::spawn(move || {
            while let Ok((mut c, _)) = l.accept() {
                let mut b = [0u8; 2048];
                let n = c.read(&mut b).unwrap_or(0);
                let req = String::from_utf8_lossy(&b[..n]).to_string();
                let acao = if req.contains("/open") { "Access-Control-Allow-Origin: *\r\n" } else if req.contains("/exact") { "Access-Control-Allow-Origin: http://evil.test\r\n" } else { "" };
                let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n{acao}Content-Length: 2\r\nConnection: close\r\n\r\nok").as_bytes());
            }
        });
        let other = "http://evil.test/page";
        assert!(fetch_req(&format!("{base}/open"), other, "GET", "", "").is_some());
        assert!(fetch_req(&format!("{base}/exact"), other, "GET", "", "").is_some());
        assert!(fetch_req(&format!("{base}/closed"), other, "GET", "", "").is_none(), "no ACAO header: response hidden from the page");
        assert!(fetch_req(&format!("{base}/open"), other, "PUT", "x", "application/json").is_none(), "needs a preflight we cannot do");
        assert!(fetch_req(&format!("{base}/open"), other, "POST", "{}", "application/json").is_none());
        assert!(fetch_req(&format!("{base}/closed"), &format!("{base}/p"), "GET", "", "").is_some(), "same origin needs no CORS header");
    }

    #[test]
    fn woff2_is_decoded_to_a_font_fontdue_can_read() {
        let packed = include_bytes!("testdata/lato.woff2");
        let ttf = decode_font(packed).expect("brotli + glyf transform");
        assert_eq!(&ttf[..4], b"\0\x01\0\0");
        assert!(ttf.len() > packed.len(), "unpacked is larger: {} vs {}", ttf.len(), packed.len());
        let font = fontdue::Font::from_bytes(ttf.as_slice(), fontdue::FontSettings::default()).expect("parses");
        let (m, bm) = font.rasterize('a', 20.0);
        assert!(m.width > 5 && bm.iter().any(|&p| p > 128), "glyph 'a' has ink");
        assert!(decode_font(b"wOF2garbage").is_none());
    }

    #[test]
    fn woff1_is_unpacked_to_sfnt_and_css_urls_become_absolute() {
        // build a one-table WOFF by hand: header (44) + 1 directory entry (20) + stored table "abcd"
        let mut w = b"wOFF\0\x01\0\0".to_vec();
        w.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 0]); // length (ignored), numTables = 1, reserved
        w.extend_from_slice(&[0, 0, 0, 4]); // totalSfntSize
        w.extend_from_slice(&[0; 12]); // version + meta/priv
        w.extend_from_slice(&[0; 12]);
        w.truncate(44);
        w.extend_from_slice(b"head");
        w.extend_from_slice(&64u32.to_be_bytes()); // offset
        w.extend_from_slice(&4u32.to_be_bytes()); // compLength
        w.extend_from_slice(&4u32.to_be_bytes()); // origLength
        w.extend_from_slice(&7u32.to_be_bytes()); // checksum
        w.extend_from_slice(b"abcd");
        let sfnt = decode_font(&w).unwrap();
        assert_eq!(&sfnt[..4], b"\0\x01\0\0");
        assert_eq!(&sfnt[12..16], b"head");
        assert_eq!(&sfnt[28..32], b"abcd");
        assert!(decode_font(b"wOF2....").is_none());
        let css = "@font-face{src:url(../f/a.woff) format('woff'),url(\"data:font/x;base64,AA==\")} .x{background:url( b.png )}";
        let out = absolutize_css_urls(css, "https://a.com/css/site.css");
        assert!(out.contains("url(\"https://a.com/f/a.woff\")") && out.contains("url(\"https://a.com/css/b.png\")") && out.contains("data:font/x;base64,AA=="), "{out}");
    }

    #[test]
    fn font_face_downloads_and_text_uses_the_family() {
        let Some(font) = ["C:\\Windows\\Fonts\\consola.ttf", "C:\\Windows\\Fonts\\arial.ttf","/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"].iter().find_map(|p| std::fs::read(p).ok()) else { return };
        let dir = std::env::temp_dir().join(format!("chrome-rs-wf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("my font.ttf"), font).unwrap();
        std::fs::write(dir.join("a.html"), "<style>@font-face{font-family:'Foo Bar';src:url('my font.ttf') format('truetype')} p{font-family:'Foo Bar',sans-serif}</style><p>Hello</p><div>plain</div>").unwrap();
        let url = format!("file:///{}/a.html", dir.to_string_lossy().replace('\\', "/").replace(' ', "%20"));
        let (_, page) = load(&url, None, None).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(page.webfonts.len(), 1, "font file was fetched through file: and decoded");
        let fam = crate::gfx::family_id("foo bar");
        assert!(page.items.iter().any(|i| matches!(i, crate::render::Item::Word { text, fam: f, .. } if text == "Hello" && *f == fam)));
        assert!(page.items.iter().any(|i| matches!(i, crate::render::Item::Word { text, fam: 0, .. } if text == "plain")));
        let mut fonts = crate::gfx::Fonts::system().unwrap();
        let before = fonts.width_f("Hello", 16.0, crate::gfx::face(false, false, false, fam));
        assert!(fonts.add_web(fam, false, false, &page.webfonts[0].data));
        let after = fonts.width_f("Hello", 16.0, crate::gfx::face(false, false, false, fam));
        assert!(before > 0.0 && after > 0.0);
    }

    fn page_text(pg: &Page) -> String {
        pg.items.iter().filter_map(|i| if let crate::render::Item::Word { text, space, .. } = i { Some(format!("{}{text}", if *space { " " } else { "" })) } else { None }).collect()
    }


    #[test]
    fn picture_and_srcset_pick_the_source_a_wide_screen_would_use() {
        assert_eq!(pick_srcset("a.jpg 480w, b.jpg 800w, c.jpg 1600w").as_deref(), Some("b.jpg"));
        assert_eq!(pick_srcset("a.jpg 320w, b.jpg 640w").as_deref(), Some("b.jpg"), "nothing wide enough: the largest");
        assert_eq!(pick_srcset("a.png, b.png 2x").as_deref(), Some("a.png"));
        assert_eq!(pick_srcset("https://c.test/r?w=1,h=2 1x, https://c.test/r?w=2 2x").as_deref(), Some("https://c.test/r?w=1,h=2"), "commas inside a url");
        assert!(media_matches("(min-width: 768px)") && !media_matches("(max-width: 600px)") && media_matches("screen"));
        let d = dom::parse("<picture><source media='(max-width:600px)' srcset='small.jpg'><source type='image/avif' srcset='x.avif'><source type='image/webp' srcset='big.webp 1x'><img src='fallback.jpg'></picture><img src='data:image/gif;base64,R0' data-srcset='lazy.jpg 1x'><img src='plain.png'>", true);
        let imgs: Vec<usize> = d.descendants().into_iter().filter(|&i| d.nodes[i].tag == "img").collect();
        assert_eq!(img_url(&d, imgs[0]).as_deref(), Some("big.webp"));
        assert_eq!(img_url(&d, imgs[1]).as_deref(), Some("lazy.jpg"));
        assert_eq!(img_url(&d, imgs[2]).as_deref(), Some("plain.png"));
    }
    #[test]
    fn live_pages_rerender_on_timers_and_clicks() {
        let dir = std::env::temp_dir().join(format!("chrome-rs-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.html"), "<p id=c>tick0</p><p id=m>idle</p><button id=b onclick=\"document.getElementById('m').textContent='clicked'; return false;\">go</button><script>var n=0; setInterval(function(){ n++; document.getElementById('c').textContent='tick'+n; }, 50); window.onload = function(){ document.getElementById('m').textContent='loaded'; };</script>").unwrap();
        let url = format!("file:///{}/a.html", dir.to_string_lossy().replace('\\', "/").replace(' ', "%20"));
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let hook: LiveHook = Box::new(move |ev| { if let LiveEvent::Page(p) = ev { let _ = tx.lock().unwrap().send(p); } });
        let (_, page) = load(&url, None, Some(hook)).unwrap();
        let handle = page.live.clone().expect("live session started");
        let first = page_text(&page);
        assert!(first.contains("loaded"), "window.onload ran before the first page: {first}");
        let btn = page.fields.iter().find(|f| f.kind == crate::render::FieldKind::Submit).unwrap().node;
        let wait = |pred: &dyn Fn(&str) -> bool| { let end = Instant::now() + Duration::from_secs(5); while Instant::now() < end { if let Ok(p) = rx.recv_timeout(Duration::from_millis(100)) { if pred(&page_text(&p)) { return true; } } } false };
        assert!(wait(&|t| t.contains("tick") && t.trim_start_matches("tick").chars().next().is_some_and(|c| c.is_ascii_digit()) && !t.contains("tick0") && !t.contains("tick1 ")), "timer updates arrive as new pages");
        assert!(handle.event(btn, "click", "{}"), "inline onclick returned false -> default prevented");
        assert!(wait(&|t| t.contains("clicked")), "click handler result is rendered");
        drop(handle);
        let _ = std::fs::remove_dir_all(&dir);
    }


    /// Adds a todo to archived TodoMVC apps (React, Vue, Svelte, jQuery...) through the live session: type, Enter, read the list back.
    /// Archives are not committed (see ROADMAP), so the test passes silently where `corpus-m4/real` is missing.
    #[test]
    #[ignore = "swaps the global archive: run alone with --ignored"]
    fn archived_todomvc_apps_accept_a_todo() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus-m4/real");
        let Ok(rd) = std::fs::read_dir(&root) else { return };
        let mut dirs: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.join("url.txt").exists()).collect();
        dirs.sort();
        let mut report = vec![];
        for d in dirs {
            let url = std::fs::read_to_string(d.join("url.txt")).unwrap();
            set_archive(Some((d.clone(), false)));
            let (tx, rx) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            let hook: LiveHook = Box::new(move |ev| { if let LiveEvent::Page(p) = ev { let _ = tx.lock().unwrap().send(p); } });
            let name = d.file_name().unwrap().to_string_lossy().to_string();
            let Ok((_, page)) = load(&url, None, Some(hook)) else { report.push(format!("{name}: load failed")); continue };
            let (Some(h), Some(input)) = (page.live.clone(), page.fields.iter().find(|f| f.kind == crate::render::FieldKind::Text).map(|f| f.node)) else { report.push(format!("{name}: no live input")); continue };
            h.value(input, "buy milk", None);
            h.post(input, "input", "{}");
            h.post(input, "keydown", "{\"key\":\"Enter\"}");
            h.post(input, "keypress", "{\"key\":\"Enter\"}");
            h.post(input, "change", "{}"); // what the UI sends when Enter commits a changed field
            h.post(input, "keyup", "{\"key\":\"Enter\"}");
            let end = Instant::now() + Duration::from_secs(3);
            let (mut ok, mut last) = (false, String::new());
            while Instant::now() < end && !ok { if let Ok(p) = rx.recv_timeout(Duration::from_millis(100)) { last = page_text(&p); ok = last.contains("buy milk"); } }
            report.push(format!("{name}: {}", if ok { "ok".to_string() } else { format!("todo did not appear; last update: {last}") }));
        }
        set_archive(None);
        println!("{}", report.join("\n"));
        assert!(report.iter().all(|r| r.ends_with(": ok")), "{}", report.join("\n"));
    }
    #[test]
    fn es_modules_import_dynamic_import_and_top_level_await() {
        let dir = std::env::temp_dir().join(format!("chrome-rs-mod-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("lib/util.js"), "export const x = 'util'; export default function twice(n) { return n * 2; }").unwrap();
        std::fs::write(dir.join("lib/dyn.js"), "export const y = 'dyn';").unwrap();
        std::fs::write(dir.join("main.js"), "import twice, { x } from './lib/util.js'; window.fromMain = x + ':' + twice(21);").unwrap();
        std::fs::write(dir.join("a.html"), "<p id=o>none</p><script type=module src=main.js></script><script type=module>import { x } from './lib/util.js'; const m = await import('./lib/dyn.js'); await Promise.resolve(); document.getElementById('o').textContent = window.fromMain + '|' + x + '|' + m.y;</script>").unwrap();
        let url = format!("file:///{}/a.html", dir.to_string_lossy().replace('\\', "/").replace(' ', "%20"));
        let (_, page) = load(&url, None, None).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(page_text(&page).contains("util:42|util|dyn"), "{}", page_text(&page));
    }
}
