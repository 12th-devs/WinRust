// Headless rendering for regression testing: `--snap in.html out.png [width]` renders one offline page to a PNG,
// `--bench DIR [runs]` prints median build/layout/paint milliseconds per page, `--corpus DIR [--bless] [--budget PCT]` renders every DIR/*.html and compares it with DIR/golden/<name>.png.
// shortcut: goldens depend on the system fonts, so they are per-machine (bless again after a font change); pages are
// rendered offline (inline CSS/JS only, no images); pages taller than MAX_H are cut off.
// Real pages: `--save DIR urls.txt [--force]` loads each URL once over the network and archives every response under DIR/NNN-name/;
// `--corpus DIR` then also loads every DIR/real/*/ from its archive (no network, full pipeline: CSS, scripts, images, fonts) and compares it with DIR/real/golden.
use crate::{gfx::{draw_field, draw_image, fill, Fonts}, layout::layout, load, render::Page};
use std::path::Path;

const MAX_H: usize = 8000;
const WIDTH: u32 = 1000;

/// Paints the whole page (no scrolling) into a `w x h` pixel buffer.
pub fn paint(page: &Page, width: u32, fonts: &mut Fonts) -> (usize, usize, Vec<u32>) {
    let (mut placed, mut rects, mut content_h) = crate::layout::layout_vh(&page.items, width as f32, 1.0, 720.0, fonts);
    // a page shorter than the viewport has its fixed boxes at the bottom of the page image instead of 720px down
    if content_h < 720.0 && placed.iter().any(|p| p.z >= crate::layout::FIXED_Z) { (placed, rects, content_h) = crate::layout::layout_vh(&page.items, width as f32, 1.0, content_h, fonts); }
    let (bw, bh) = (width as usize, (content_h.ceil() as usize).clamp(1, MAX_H));
    let mut buf = vec![page.bg; bw * bh];
    let groups = crate::layout::groups(&rects);
    let clip = |grp: u32| groups.get(&grp).map(|g| (g.x as i32, g.y as i32, (g.x + g.w).ceil() as i32, (g.y + g.h).ceil() as i32));
    for (_, rr, pr) in crate::layout::layers(&placed, &rects) {
        for r in &rects[rr.clone()] {
            crate::gfx::set_clip(clip(r.grp).filter(|_| r.scroll.is_nan()));
            crate::gfx::draw_rect(&mut buf, bw, bh, r, 0.0);
            if let Some(pic) = &r.pic { crate::gfx::draw_pic(&mut buf, bw, bh, &page.images[pic.img], r, 0.0); }
        }
        for p in &placed[pr] {
            crate::gfx::set_clip(clip(p.grp));
            if let Some(i) = p.img { draw_image(&mut buf, bw, bh, &page.images[i], p.x, p.base - p.size, p.w, p.size, if p.color == 0 { 255 } else { p.color }); continue; }
            if let Some(i) = p.field { draw_field(&mut buf, bw, bh, fonts, &page.fields[i], None, 1.0, p.x, p.base - p.size, p.w, p.size); continue; }
            crate::gfx::draw_text_ls(&mut buf, bw, bh, fonts, &p.text, p.x, p.base, p.size, p.color, p.face, p.ls);
            if p.ul { fill(&mut buf, bw, p.x as i32, p.base as i32 + 2, p.w as i32, 1, p.color); }
        }
        crate::gfx::set_clip(None);
        for r in rects[rr].iter().filter(|r| r.scroll.is_finite()) { crate::gfx::draw_box_scrollbar(&mut buf, bw, bh, r.x, r.y, r.w, r.h, r.scroll, 0.0, 1.0); }
    }
    (bw, bh, buf)
}

/// Minimal PDF: the page image cut into A4 sheets (one Flate-compressed RGB image per sheet, full width). Not selectable text.
pub fn pdf(buf: &[u32], w: usize, h: usize) -> Vec<u8> {
    const PW: f32 = 595.0; // A4 in points
    const PH: f32 = 842.0;
    let sheet_px = ((PH / PW) * w as f32).floor().max(1.0) as usize; // source rows per sheet
    let sheets = h.div_ceil(sheet_px).max(1);
    let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut offs: Vec<usize> = vec![];
    let mut obj = |out: &mut Vec<u8>, body: &[u8]| { offs.push(out.len()); out.extend_from_slice(format!("{} 0 obj\n", offs.len()).as_bytes()); out.extend_from_slice(body); out.extend_from_slice(b"\nendobj\n"); };
    // objects: 1 catalog, 2 pages, then per sheet: page, contents, image
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: String = (0..sheets).map(|i| format!("{} 0 R ", 3 + i * 3)).collect();
    obj(&mut out, format!("<< /Type /Pages /Count {sheets} /Kids [{kids}] >>").as_bytes());
    for i in 0..sheets {
        let (y0, y1) = (i * sheet_px, ((i + 1) * sheet_px).min(h));
        let rows = y1 - y0;
        let ph = PW * rows as f32 / w as f32; // image height on the sheet in points
        let (page, content, image) = (3 + i * 3, 4 + i * 3, 5 + i * 3);
        obj(&mut out, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PW} {PH}] /Contents {content} 0 R /Resources << /XObject << /Im0 {image} 0 R >> >> >>").as_bytes());
        let _ = page;
        let c = format!("q {PW} 0 0 {ph} 0 {} cm /Im0 Do Q", PH - ph);
        obj(&mut out, format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()).as_bytes());
        let raw: Vec<u8> = buf[y0 * w..y1 * w].iter().flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8]).collect();
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
        let mut body = format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {rows} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n", z.len()).into_bytes();
        body.extend_from_slice(&z);
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &body);
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offs.len() + 1).as_bytes());
    for o in &offs { out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes()); }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offs.len() + 1).as_bytes());
    out
}

fn save_png(path: &Path, w: usize, h: usize, buf: &[u32]) -> Result<(), String> {
    let rgb: Vec<u8> = buf.iter().flat_map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]).collect();
    image::save_buffer(path, &rgb, w as u32, h as u32, image::ColorType::Rgb8).map_err(|e| e.to_string())
}

/// Percentage of pixels that differ; the area outside the overlap counts as different.
fn diff_pct(a: (usize, usize, &[u32]), b: (usize, usize, &[u32])) -> f64 {
    let (w, h) = (a.0.max(b.0), a.1.max(b.1));
    let same = (0..a.1.min(b.1)).map(|y| (0..a.0.min(b.0)).filter(|&x| a.2[y * a.0 + x] == b.2[y * b.0 + x]).count()).sum::<usize>();
    100.0 * (1.0 - same as f64 / (w * h) as f64)
}

fn render_file(path: &Path, fonts: &mut Fonts) -> Result<(usize, usize, Vec<u32>), String> {
    let html = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let page = load::load_offline(&html, "https://corpus.test/");
    Ok(paint(&page, WIDTH, fonts))
}

fn html_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut pages: Vec<_> = std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "html")).collect();
    pages.sort();
    Ok(pages)
}

fn load_golden(path: &Path) -> Option<(usize, usize, Vec<u32>)> {
    let im = image::open(path).ok()?.to_rgb8();
    let buf = im.pixels().map(|p| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32).collect();
    Some((im.width() as usize, im.height() as usize, buf))
}

/// Entry for `--snap` / `--corpus`; returns the process exit code.
pub fn cli(args: &[String], fonts: &mut Fonts) -> i32 {
    // cookies and local storage of the pages loaded here go to a scratch directory, never into the user's browser profile
    // SAFETY: the main thread only waits for this one, no other thread reads the environment yet
    unsafe { std::env::set_var("CHROME_RS_DATA", std::env::temp_dir().join("chrome-rs-corpus-data")); }
    let mut run = || -> Result<i32, String> {
        match args.first().map(String::as_str) {
            Some("--pdf") => {
                let (inp, out) = (args.get(1).ok_or("usage: --pdf in.html out.pdf")?, args.get(2).ok_or("usage: --pdf in.html out.pdf")?);
                let html = std::fs::read_to_string(inp).map_err(|e| e.to_string())?;
                let (w, h, buf) = paint(&load::load_offline(&html, "https://corpus.test/"), 794, fonts);
                std::fs::write(out, pdf(&buf, w, h)).map_err(|e| e.to_string())?;
                println!("{out}: {w}x{h} px");
                Ok(0)
            }
            Some("--snap") => {
                let (inp, out) = (args.get(1).ok_or("usage: --snap in.html out.png [width]")?, args.get(2).ok_or("usage: --snap in.html out.png [width]")?);
                let html = std::fs::read_to_string(inp).map_err(|e| e.to_string())?;
                let width = args.get(3).and_then(|w| w.parse().ok()).unwrap_or(WIDTH);
                let (w, h, buf) = paint(&load::load_offline(&html, "https://corpus.test/"), width, fonts);
                save_png(Path::new(out), w, h, &buf)?;
                println!("{out}: {w}x{h}");
                Ok(0)
            }
            Some("--bench") => {
                let dir = Path::new(args.get(1).ok_or("usage: --bench DIR [runs]")?);
                let runs: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(20).max(1);
                println!("{:<16} {:>9} {:>9} {:>9}", "page", "build ms", "layout ms", "paint ms");
                for p in html_files(dir)? {
                    let html = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
                    let (mut b, mut l, mut t) = (vec![], vec![], vec![]);
                    for _ in 0..runs {
                        let t0 = std::time::Instant::now();
                        let page = load::load_offline(&html, "https://corpus.test/");
                        b.push(t0.elapsed().as_secs_f64() * 1e3);
                        let t1 = std::time::Instant::now();
                        let _ = layout(&page.items, WIDTH as f32, 1.0, fonts);
                        l.push(t1.elapsed().as_secs_f64() * 1e3);
                        let t2 = std::time::Instant::now();
                        let _ = paint(&page, WIDTH, fonts);
                        t.push(t2.elapsed().as_secs_f64() * 1e3 - l.last().copied().unwrap_or(0.0));
                    }
                    let med = |v: &mut Vec<f64>| { v.sort_by(|a, b| a.total_cmp(b)); v[v.len() / 2] };
                    println!("{:<16} {:>9.2} {:>9.2} {:>9.2}", p.file_stem().and_then(|s| s.to_str()).unwrap_or("?"), med(&mut b), med(&mut l), med(&mut t).max(0.0));
                }
                Ok(0)
            }
            Some("--save") => {
                let usage = "usage: --save DIR urls.txt [--force]";
                let (dir, list) = (Path::new(args.get(1).ok_or(usage)?), args.get(2).ok_or(usage)?);
                let force = args.iter().any(|a| a == "--force");
                let urls: Vec<String> = std::fs::read_to_string(list).map_err(|e| e.to_string())?.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect();
                let mut ok = 0;
                for (i, u) in urls.iter().enumerate() {
                    let slug: String = u.split("://").nth(1).unwrap_or(u).chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).take(40).collect();
                    let d = dir.join(format!("{:03}-{}", i + 1, slug.trim_matches('-')));
                    if d.join("url.txt").exists() && !force { ok += 1; continue; }
                    let _ = std::fs::remove_dir_all(&d);
                    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
                    load::set_archive(Some((d.clone(), true)));
                    match load::load(u, None, None) {
                        Ok((_, page)) if !page.items.is_empty() => { std::fs::write(d.join("url.txt"), u).map_err(|e| e.to_string())?; println!("saved {} ({} items)", d.file_name().and_then(|n| n.to_str()).unwrap_or("?"), page.items.len()); ok += 1; }
                        Ok(_) => { println!("EMPTY {u}"); let _ = std::fs::remove_dir_all(&d); }
                        Err(e) => { println!("FAIL  {u}: {e}"); let _ = std::fs::remove_dir_all(&d); }
                    }
                }
                load::set_archive(None);
                println!("{ok} of {} pages saved", urls.len());
                Ok(0)
            }
            _ => {
                let dir = Path::new(args.get(1).ok_or("usage: --corpus DIR [--bless] [--budget PCT]")?);
                let bless = args.iter().any(|a| a == "--bless");
                let budget: f64 = args.iter().position(|a| a == "--budget").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(0.5);
                let (out_dir, gold_dir) = (dir.join("out"), dir.join("golden"));
                std::fs::create_dir_all(&out_dir).and_then(|_| std::fs::create_dir_all(&gold_dir)).map_err(|e| e.to_string())?;
                let pages = html_files(dir)?;
                let mut failed = 0;
                // real pages (see --save) come from their archives; a page that fails to load counts as over budget
                let real = dir.join("real");
                let mut archives: Vec<std::path::PathBuf> = std::fs::read_dir(&real).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.join("url.txt").exists()).collect()).unwrap_or_default();
                archives.sort();
                let (real_out, real_gold) = (real.join("out"), real.join("golden"));
                if !archives.is_empty() { std::fs::create_dir_all(&real_out).and_then(|_| std::fs::create_dir_all(&real_gold)).map_err(|e| e.to_string())?; }
                let jobs = pages.iter().map(|p| (p.clone(), false)).chain(archives.iter().map(|p| (p.clone(), true)));
                let total = pages.len() + archives.len();
                for (p, is_real) in jobs {
                    let name = p.file_stem().and_then(|s| s.to_str()).or_else(|| p.file_name().and_then(|s| s.to_str())).unwrap_or("page").to_string();
                    let rendered = if is_real {
                        let url = std::fs::read_to_string(p.join("url.txt")).map_err(|e| e.to_string())?;
                        load::set_archive(Some((p.clone(), false)));
                        let r = load::load(url.trim(), None, None).map(|(_, page)| paint(&page, WIDTH, fonts));
                        load::set_archive(None);
                        match r { Ok(r) => r, Err(e) => { failed += 1; println!("ERROR {name}: {e}"); continue; } }
                    } else { render_file(&p, fonts)? };
                    let (w, h, buf) = rendered;
                    let (out_dir, gold_dir) = if is_real { (&real_out, &real_gold) } else { (&out_dir, &gold_dir) };
                    let out_png = out_dir.join(format!("{name}.png"));
                    save_png(&out_png, w, h, &buf)?;
                    let gold = gold_dir.join(format!("{name}.png"));
                    if bless { std::fs::copy(&out_png, &gold).map_err(|e| e.to_string())?; println!("BLESS {name} {w}x{h}"); continue; }
                    match load_golden(&gold) {
                        None => println!("NEW   {name} (no golden yet; run with --bless)"),
                        Some((gw, gh, gb)) => {
                            let d = diff_pct((w, h, &buf), (gw, gh, &gb));
                            let ok = d <= budget;
                            if !ok { failed += 1; }
                            println!("{} {name} {d:.3}% (budget {budget}%)", if ok { "ok   " } else { "FAIL " });
                        }
                    }
                }
                println!("{total} pages ({} real), {failed} over budget", archives.len());
                Ok((failed > 0) as i32)
            }
        }
    };
    run().unwrap_or_else(|e| { eprintln!("error: {e}"); 2 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_has_one_sheet_per_a4_slice_and_a_valid_xref() {
        let buf = vec![0x336699u32; 100 * 300]; // 100 x 300 px -> sheet height 100*842/595 = 141 px -> 3 sheets
        let p = pdf(&buf, 100, 300);
        let rfind = |needle: &[u8]| p.windows(needle.len()).rposition(|w| w == needle).unwrap();
        assert!(p.starts_with(b"%PDF-1.4") && p.trim_ascii_end().ends_with(b"%%EOF"));
        assert!(p.windows(8).any(|w| w == b"/Count 3"), "three sheets");
        // every xref offset points at "N 0 obj"
        let sx = rfind(b"startxref\n") + 10;
        let start: usize = std::str::from_utf8(&p[sx..]).unwrap().lines().next().unwrap().parse().unwrap();
        assert_eq!(&p[start..start + 4], b"xref");
        let table = std::str::from_utf8(&p[start..rfind(b"trailer")]).unwrap();
        let entries: Vec<usize> = table.lines().skip(2).filter(|l| l.ends_with(" n ")).map(|l| l[..10].parse().unwrap()).collect();
        assert_eq!(entries.len(), 2 + 3 * 3);
        for (i, o) in entries.iter().enumerate() { assert!(p[*o..].starts_with(format!("{} 0 obj", i + 1).as_bytes()), "object {}", i + 1); }
    }

    #[test]
    fn diff_counts_changed_and_missing_pixels() {
        let a = vec![0u32; 4];
        assert_eq!(diff_pct((2, 2, &a), (2, 2, &a)), 0.0);
        assert_eq!(diff_pct((2, 2, &a), (2, 2, &[0, 0, 0, 1])), 25.0);
        assert_eq!(diff_pct((2, 2, &a), (2, 1, &a[..2])), 50.0); // missing row counts as different
    }
}
