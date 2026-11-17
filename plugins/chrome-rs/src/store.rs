// Persistent state: the cookie jar (RFC 6265 subset) and per-origin localStorage, both kept under the data dir.
// shortcut: `Expires` is ignored (cookies without Max-Age live for the session), no SameSite/public-suffix checks, no size caps per
// cookie; upgrade when third-party isolation (M8) lands.
use std::{path::PathBuf, sync::{Mutex, OnceLock}, time::{SystemTime, UNIX_EPOCH}};

#[derive(Clone, Debug, PartialEq)]
pub struct Cookie { name: String, value: String, domain: String, host_only: bool, path: String, secure: bool, http_only: bool, expires: Option<u64> }

#[derive(Default)]
pub struct Jar { cookies: Vec<Cookie> }

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) }

/// (scheme, host without port, path without query) of an absolute URL.
pub fn split_url(url: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = &rest[..end];
    let host = auth.rsplit('@').next().unwrap_or(auth);
    let host = host.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(host, |(h, _)| h);
    let path = rest[end..].split(['?', '#']).next().unwrap_or("");
    Some((scheme, host, if path.is_empty() { "/" } else { path }))
}

/// `scheme://host[:port]` of an absolute URL ("" when there is none).
pub fn origin_of(url: &str) -> String {
    let end = url.find("://").map_or(0, |p| url[p + 3..].find(['/', '?', '#']).map_or(url.len(), |e| p + 3 + e));
    url[..end].to_string()
}

/// Same site, loosely: equal hosts or one a subdomain of the other.
pub fn same_site(a: &str, b: &str) -> bool { a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}")) }

fn domain_match(host: &str, c: &Cookie) -> bool { if c.host_only { host == c.domain } else { host == c.domain || host.ends_with(&format!(".{}", c.domain)) } }

fn path_match(req: &str, cp: &str) -> bool { req == cp || req.starts_with(cp) && (cp.ends_with('/') || req[cp.len()..].starts_with('/')) }

impl Jar {
    /// Stores one `Set-Cookie` header received from `url`. `from_js` rejects HttpOnly (scripts cannot create those).
    pub fn set(&mut self, url: &str, header: &str, from_js: bool) {
        let Some((_, host, rpath)) = split_url(url) else { return };
        let mut parts = header.split(';');
        let Some((name, value)) = parts.next().and_then(|nv| nv.split_once('=')) else { return };
        let name = name.trim();
        if name.is_empty() { return; }
        let default_path = match rpath.rfind('/') { Some(0) | None => "/".to_string(), Some(i) => rpath[..i].to_string() };
        let mut c = Cookie { name: name.into(), value: value.trim().into(), domain: host.to_ascii_lowercase(), host_only: true, path: default_path, secure: false, http_only: false, expires: None };
        let mut delete = false;
        for attr in parts {
            let (k, v) = attr.split_once('=').map_or((attr.trim(), ""), |(k, v)| (k.trim(), v.trim()));
            match k.to_ascii_lowercase().as_str() {
                "domain" => {
                    let d = v.trim_start_matches('.').to_ascii_lowercase();
                    if !d.is_empty() && (host == d || host.ends_with(&format!(".{d}"))) && d.contains('.') { c.domain = d; c.host_only = false; } else { return; }
                }
                "path" => if v.starts_with('/') { c.path = v.into() },
                "secure" => c.secure = true,
                "httponly" => c.http_only = true,
                "max-age" => if let Ok(n) = v.parse::<i64>() { if n <= 0 { delete = true } else { c.expires = Some(now() + n as u64) } },
                _ => {}
            }
        }
        if from_js && c.http_only { return; }
        self.cookies.retain(|o| !(o.name == c.name && o.domain == c.domain && o.path == c.path));
        if !delete { self.cookies.push(c); }
    }

    /// Value of the `Cookie` request header for `url` ("" when nothing matches).
    pub fn header(&mut self, url: &str, with_http_only: bool) -> String {
        let Some((scheme, host, path)) = split_url(url) else { return String::new() };
        let t = now();
        self.cookies.retain(|c| c.expires.is_none_or(|e| e > t));
        let mut hit: Vec<&Cookie> = self.cookies.iter()
            .filter(|c| domain_match(host, c) && path_match(path, &c.path) && (!c.secure || scheme == "https") && (with_http_only || !c.http_only)).collect();
        hit.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        hit.iter().map(|c| format!("{}={}", c.name, c.value)).collect::<Vec<_>>().join("; ")
    }

    fn to_tsv(&self) -> String {
        self.cookies.iter().filter_map(|c| Some(format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", c.domain, c.host_only as u8, c.path, c.secure as u8, c.http_only as u8, c.expires?, c.name, c.value))).collect()
    }

    fn from_tsv(s: &str) -> Jar {
        let cookies = s.lines().filter_map(|l| {
            let f: Vec<&str> = l.splitn(8, '\t').collect();
            if f.len() < 8 { return None; }
            Some(Cookie { domain: f[0].into(), host_only: f[1] == "1", path: f[2].into(), secure: f[3] == "1", http_only: f[4] == "1", expires: f[5].parse().ok(), name: f[6].into(), value: f[7].into() })
        }).collect();
        Jar { cookies }
    }
}

pub fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("CHROME_RS_DATA").map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("chrome-rs")))
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".chrome-rs")))?;
    std::fs::create_dir_all(&base).ok()?;
    Some(base)
}

fn jar() -> &'static Mutex<Jar> {
    static JAR: OnceLock<Mutex<Jar>> = OnceLock::new();
    JAR.get_or_init(|| Mutex::new(data_dir().and_then(|d| std::fs::read_to_string(d.join("cookies.tsv")).ok()).map_or_else(Jar::default, |s| Jar::from_tsv(&s))))
}

/// Cookie request header for `url` from the shared jar.
pub fn cookie_header(url: &str, with_http_only: bool) -> String { jar().lock().map(|mut j| j.header(url, with_http_only)).unwrap_or_default() }

/// Records a `Set-Cookie` header in the shared jar and persists the persistent cookies.
pub fn set_cookie(url: &str, header: &str, from_js: bool) {
    let Ok(mut j) = jar().lock() else { return };
    j.set(url, header, from_js);
    if let Some(d) = data_dir() { let _ = std::fs::write(d.join("cookies.tsv"), j.to_tsv()); }
}

fn ls_file(dir: &std::path::Path) -> PathBuf { dir.join("localstorage.json") }

/// localStorage of `origin` as a JSON object string.
pub fn ls_load_in(dir: &std::path::Path, origin: &str) -> String {
    let all: serde_json::Value = std::fs::read_to_string(ls_file(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    all.get(origin).filter(|v| v.is_object()).map_or_else(|| "{}".into(), |v| v.to_string())
}

/// Replaces the stored localStorage of `origin` (ignored when not a JSON object or over 5 MB).
pub fn ls_save_in(dir: &std::path::Path, origin: &str, json: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return };
    if !v.is_object() || json.len() > 5_000_000 { return; }
    let mut all: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(ls_file(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if v.as_object().is_some_and(|o| o.is_empty()) { all.remove(origin); } else { all.insert(origin.into(), v); }
    let _ = std::fs::write(ls_file(dir), serde_json::Value::Object(all).to_string());
}

pub fn ls_load(origin: &str) -> String { data_dir().map_or_else(|| "{}".into(), |d| ls_load_in(&d, origin)) }
pub fn ls_save(origin: &str, json: &str) { if let Some(d) = data_dir() { ls_save_in(&d, origin, json); } }

// ---------- history, bookmarks, session, settings (tab-separated text files in the data dir)

fn read_lines(dir: &std::path::Path, file: &str) -> Vec<String> { std::fs::read_to_string(dir.join(file)).map(|s| s.lines().map(String::from).collect()).unwrap_or_default() }

fn clean(s: &str) -> String { s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string() }

/// One saved page: address, title and unix time.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry { pub url: String, pub title: String, pub time: u64 }

fn parse_entry(l: &str) -> Option<Entry> {
    let mut f = l.splitn(3, '\t');
    Some(Entry { url: f.next()?.to_string(), title: f.next()?.to_string(), time: f.next()?.parse().ok()? })
}

fn entries_in(dir: &std::path::Path, file: &str) -> Vec<Entry> { read_lines(dir, file).iter().filter_map(|l| parse_entry(l)).collect() }

fn write_entries(dir: &std::path::Path, file: &str, e: &[Entry]) {
    let _ = std::fs::write(dir.join(file), e.iter().map(|x| format!("{}\t{}\t{}\n", x.url, x.title, x.time)).collect::<String>());
}

/// Whether a page belongs in history / bookmarks (not internal, scripted or data pages).
pub fn recordable(url: &str) -> bool { url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://") }

/// Appends a visit (a repeat of the previous visit only updates it); the file is trimmed to the newest 2000 entries.
pub fn add_history_in(dir: &std::path::Path, url: &str, title: &str) {
    if !recordable(url) { return; }
    let mut h = entries_in(dir, "history.tsv");
    let e = Entry { url: clean(url), title: clean(title), time: now() };
    if h.last().is_some_and(|l| l.url == e.url) { h.pop(); }
    h.push(e);
    if h.len() > 2000 { h.drain(..h.len() - 2000); }
    write_entries(dir, "history.tsv", &h);
}

/// Newest first.
pub fn history_in(dir: &std::path::Path) -> Vec<Entry> { let mut h = entries_in(dir, "history.tsv"); h.reverse(); h }

#[cfg(test)]
pub fn clear_history_in(dir: &std::path::Path) { let _ = std::fs::remove_file(dir.join("history.tsv")); }

pub fn bookmarks_in(dir: &std::path::Path) -> Vec<Entry> { entries_in(dir, "bookmarks.tsv") }

pub fn is_bookmarked_in(dir: &std::path::Path, url: &str) -> bool { bookmarks_in(dir).iter().any(|b| b.url == url) }

/// Adds the page, or removes it when it is already bookmarked; returns true when it is bookmarked afterwards.
pub fn toggle_bookmark_in(dir: &std::path::Path, url: &str, title: &str) -> bool {
    if !recordable(url) { return false; }
    let mut b = bookmarks_in(dir);
    let had = b.len();
    b.retain(|x| x.url != url);
    let added = b.len() == had;
    if added { b.push(Entry { url: clean(url), title: clean(title), time: now() }); }
    write_entries(dir, "bookmarks.tsv", &b);
    added
}

/// Address-bar suggestions: bookmarks first, then history, whose url or title contains every word of `q` (case-insensitive).
pub fn suggest_in(dir: &std::path::Path, q: &str, n: usize) -> Vec<Entry> {
    let words: Vec<String> = q.to_lowercase().split_whitespace().map(String::from).collect();
    if words.is_empty() { return vec![]; }
    let mut out: Vec<Entry> = vec![];
    for e in bookmarks_in(dir).into_iter().chain(history_in(dir)) {
        let hay = format!("{} {}", e.url, e.title).to_lowercase();
        if words.iter().all(|w| hay.contains(w)) && !out.iter().any(|o| o.url == e.url) { out.push(e); }
        if out.len() >= n { break; }
    }
    out
}

pub fn save_session_in(dir: &std::path::Path, urls: &[String], current: usize) {
    let body: String = std::iter::once(format!("{current}\n")).chain(urls.iter().filter(|u| recordable(u)).map(|u| format!("{}\n", clean(u)))).collect();
    let _ = std::fs::write(dir.join("session.txt"), body);
}

/// (tab urls, index of the active tab) of the previous run.
pub fn load_session_in(dir: &std::path::Path) -> (Vec<String>, usize) {
    let l = read_lines(dir, "session.txt");
    let cur = l.first().and_then(|c| c.parse().ok()).unwrap_or(0);
    let urls: Vec<String> = l.into_iter().skip(1).filter(|u| recordable(u)).collect();
    let cur = cur.min(urls.len().saturating_sub(1));
    (urls, cur)
}

/// Value of a `key<TAB>value` line in settings.tsv.
pub fn setting_in(dir: &std::path::Path, key: &str) -> Option<String> {
    read_lines(dir, "settings.tsv").iter().find_map(|l| l.split_once('\t').filter(|(k, _)| *k == key).map(|(_, v)| v.trim().to_string())).filter(|v| !v.is_empty())
}

pub fn add_history(url: &str, title: &str) { if let Some(d) = data_dir() { add_history_in(&d, url, title); } }
pub fn toggle_bookmark(url: &str, title: &str) -> bool { data_dir().is_some_and(|d| toggle_bookmark_in(&d, url, title)) }
pub fn is_bookmarked(url: &str) -> bool { data_dir().is_some_and(|d| is_bookmarked_in(&d, url)) }
pub fn suggest(q: &str, n: usize) -> Vec<Entry> { data_dir().map_or_else(Vec::new, |d| suggest_in(&d, q, n)) }
pub fn save_session(urls: &[String], current: usize) { if let Some(d) = data_dir() { save_session_in(&d, urls, current); } }
pub fn load_session() -> (Vec<String>, usize) { data_dir().map_or_else(|| (vec![], 0), |d| load_session_in(&d)) }
pub fn setting(key: &str) -> Option<String> { data_dir().and_then(|d| setting_in(&d, key)) }

fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }

fn day(t: u64) -> String {
    // civil date from unix days (UTC)
    let z = (t / 86_400) as i64 + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let (y, doy) = (yoe + era * 400, doe - (365 * yoe + yoe / 4 - yoe / 100));
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// HTML of the internal `about:` pages that list stored data.
pub fn about_html_in(dir: &std::path::Path, page: &str) -> Option<String> {
    let list = |title: &str, e: &[Entry], empty: &str| {
        let mut h = format!("<title>{title}</title><h2>{title}</h2>");
        if e.is_empty() { h += &format!("<p>{empty}</p>"); }
        let mut last = String::new();
        for x in e {
            let d = day(x.time);
            if d != last { h += &format!("<h4>{d}</h4>"); last = d; }
            h += &format!("<div><a href=\"{}\">{}</a> <small>{}</small></div>", esc(&x.url), esc(if x.title.is_empty() { &x.url } else { &x.title }), esc(&x.url));
        }
        h
    };
    Some(match page {
        "about:history" => list("History", &history_in(dir), "Nothing visited yet."),
        "about:bookmarks" => list("Bookmarks", &bookmarks_in(dir), "No bookmarks yet. Press Ctrl+D on a page to add one."),
        "about:settings" => {
            let rows: String = [("home", "Home page (first tab)", "https://example.com"), ("search", "Search engine, {q} is the query", "https://html.duckduckgo.com/html/?q={q}")].iter()
                .map(|(k, d, def)| format!("<tr><td><b>{k}</b></td><td>{}</td><td>{}</td></tr>", esc(d), esc(&setting_in(dir, k).unwrap_or_else(|| format!("{def} (default)"))))).collect();
            format!("<title>Settings</title><h2>Settings</h2><p>Edit <code>{}</code> (one <code>key&lt;TAB&gt;value</code> per line), then restart.</p><table border=1 cellpadding=4>{rows}</table>", esc(&dir.join("settings.tsv").to_string_lossy()))
        }
        _ => return None,
    })
}

pub fn about_html(page: &str) -> Option<String> { about_html_in(&data_dir()?, page) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_scope_by_domain_path_secure_and_expiry() {
        let mut j = Jar::default();
        j.set("https://www.a.com/x/y", "sid=1; Path=/x; HttpOnly", false);
        j.set("https://www.a.com/", "all=2; Domain=a.com", false);
        j.set("https://www.a.com/", "sec=3; Secure", false);
        assert_eq!(j.header("https://www.a.com/x/z", true), "sid=1; all=2; sec=3");
        assert_eq!(j.header("https://www.a.com/x/z", false), "all=2; sec=3"); // scripts do not see HttpOnly
        assert_eq!(j.header("https://api.a.com/", true), "all=2"); // domain cookie reaches subdomains, host-only ones do not
        assert_eq!(j.header("http://www.a.com/", true), "all=2"); // Secure needs https
        assert_eq!(j.header("https://www.a.com/xy", true), "all=2; sec=3"); // path must match on a segment boundary
        assert_eq!(j.header("https://evil.com/", true), "");
        j.set("https://www.a.com/", "all=; Domain=a.com; Max-Age=0", false);
        assert_eq!(j.header("https://api.a.com/", true), "");
        j.set("https://www.a.com/", "x=1; Domain=b.com", false); // foreign Domain is rejected
        assert_eq!(j.header("https://b.com/", true), "");
        j.set("https://www.a.com/", "h=1; HttpOnly", true); // script cannot set HttpOnly
        assert!(!j.header("https://www.a.com/", true).contains("h=1"));
    }

    #[test]
    fn persistent_cookies_roundtrip_and_session_ones_stay_out() {
        let mut j = Jar::default();
        j.set("https://a.com/", "keep=1; Max-Age=3600", false);
        j.set("https://a.com/", "tmp=2", false);
        let tsv = j.to_tsv();
        assert!(tsv.contains("keep") && !tsv.contains("tmp"));
        assert_eq!(Jar::from_tsv(&tsv).header("https://a.com/", true), "keep=1");
    }

    #[test]
    fn url_split_and_same_site() {
        assert_eq!(split_url("https://u:p@a.com:8080/x/y?q#f"), Some(("https", "a.com", "/x/y")));
        assert_eq!(split_url("https://a.com"), Some(("https", "a.com", "/")));
        assert!(same_site("www.a.com", "a.com") && !same_site("a.com", "b.com"));
        assert_eq!(origin_of("https://a.com:8080/x?y#z"), "https://a.com:8080");
        assert_ne!(origin_of("http://a.com/"), origin_of("https://a.com/"));
    }

    #[test]
    fn local_storage_persists_per_origin() {
        let dir = std::env::temp_dir().join(format!("chrome-rs-ls-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(ls_load_in(&dir, "https://a.com"), "{}");
        ls_save_in(&dir, "https://a.com", r#"{"k":"v"}"#);
        ls_save_in(&dir, "https://b.com", r#"{"x":"y"}"#);
        assert_eq!(ls_load_in(&dir, "https://a.com"), r#"{"k":"v"}"#);
        ls_save_in(&dir, "https://a.com", "{}");
        assert_eq!(ls_load_in(&dir, "https://a.com"), "{}");
        assert_eq!(ls_load_in(&dir, "https://b.com"), r#"{"x":"y"}"#);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("chrome-rs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn history_bookmarks_and_suggestions() {
        let d = tmp("hist");
        add_history_in(&d, "https://a.com/x", "Alpha page");
        add_history_in(&d, "about:blank", "ignored");
        add_history_in(&d, "https://b.com/", "Beta\tpage");
        add_history_in(&d, "https://b.com/", "Beta again"); // repeat of the last visit replaces it
        let h = history_in(&d);
        assert_eq!(h.iter().map(|e| (e.url.as_str(), e.title.as_str())).collect::<Vec<_>>(), vec![("https://b.com/", "Beta again"), ("https://a.com/x", "Alpha page")]);
        assert!(toggle_bookmark_in(&d, "https://a.com/x", "Alpha page") && is_bookmarked_in(&d, "https://a.com/x"));
        assert_eq!(suggest_in(&d, "alp", 5).len(), 1);
        assert_eq!(suggest_in(&d, "b.com beta", 5)[0].url, "https://b.com/");
        assert_eq!(suggest_in(&d, "zzz", 5).len(), 0);
        assert_eq!(suggest_in(&d, "a", 1).len(), 1);
        assert!(!toggle_bookmark_in(&d, "https://a.com/x", "") && !is_bookmarked_in(&d, "https://a.com/x")); // toggles off
        assert!(!toggle_bookmark_in(&d, "javascript:alert(1)", "x"));
        let page = about_html_in(&d, "about:history").unwrap();
        assert!(page.contains("<a href=\"https://b.com/\">Beta again</a>") && page.contains("<h4>20"));
        assert!(about_html_in(&d, "about:bookmarks").unwrap().contains("No bookmarks yet"));
        clear_history_in(&d);
        assert!(history_in(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn session_and_settings_files() {
        let d = tmp("sess");
        save_session_in(&d, &["https://a.com/".into(), "about:blank".into(), "https://b.com/".into()], 1);
        assert_eq!(load_session_in(&d), (vec!["https://a.com/".to_string(), "https://b.com/".to_string()], 1));
        assert_eq!(load_session_in(&tmp("none")), (vec![], 0));
        std::fs::write(d.join("settings.tsv"), "home\thttps://start.test/\nsearch\t\n").unwrap();
        assert_eq!(setting_in(&d, "home").as_deref(), Some("https://start.test/"));
        assert_eq!(setting_in(&d, "search"), None);
        assert!(about_html_in(&d, "about:settings").unwrap().contains("https://start.test/"));
        assert_eq!(day(0), "1970-01-01");
        assert_eq!(day(1_800_000_000), "2027-01-15");
        let _ = std::fs::remove_dir_all(&d);
    }
}
