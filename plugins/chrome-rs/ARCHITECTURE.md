# chrome-rs architecture

One process, one worker thread per page load, CPU rendering. Data flows one way:

```
load.rs   fetch main HTML (manual redirects, cookies) -> dom.rs parse
          -> CSS/JS/images fetched in parallel (same-site cookies only)
          -> js.rs  QuickJS runs scripts + timers (virtual time, 4 s CPU budget), DOM read back
          -> render.rs + css.rs  cascade -> flat Item stream (words, boxes, tables, floats, flex, abs, inline-blocks)
          live pages: js.rs session thread (QuickJS stays alive) <-> UI events; DOM changes -> builder thread -> new Page
main.rs   UI thread: layout.rs (Item stream -> Placed text/pictures/controls + background Rects) -> paint (gfx.rs)
snap.rs   headless paint of the same pipeline for regression tests and timing
```

## Modules
| File | Role |
|------|------|
| `dom.rs` | HTML parser, arena DOM (`Dom`, `Node`), JSON round trip with JS |
| `css.rs` | Selector parser, `Sheet` (indexed rules), `compute()` cascade into `Style` (`Ext` holds non-inherited layout props) |
| `render.rs` | Styled DOM -> `Item` stream; forms/fields; tables; flex children; float/inline-block/absolute wrapping |
| `layout.rs` | Flow layout: lines, blocks, floats, tables, inline-block atoms, row/column flex, grid, relative/absolute/fixed positioning, paint layers (`z`), UAX #9 line reordering |
| `gfx.rs` | Fonts (fontdue for the main faces, ttf-parser outlines for coverage fallbacks), glyph cache, fill/blend/gradient/image/text/field painting |
| `js.rs` + `prelude.js` | QuickJS runtime and the JS-side DOM/BOM; natives for fetch, cookies, localStorage |
| `store.rs` | Cookie jar (RFC 6265 subset) and per-origin localStorage, persisted under the data dir |
| `load.rs` | Network pipeline, redirects, charset decoding, image decode |
| `snap.rs` | `--snap`, `--pdf`, `--corpus`, `--bench` command-line modes; PDF writer |
| `browse.rs` | Find-in-page, text selection, hit-testing, node boxes for JS (window-independent, unit-tested) |

## Decisions
- **Item stream, not a box tree.** Layout walks a flat list with Box/BoxEnd markers; floats, inline-blocks, absolutes and flex items are nested item lists laid out by recursive `block()` calls. Cheap and fast, but vertical padding/margins are folded into `Break` gaps, so `height`, vertical padding backgrounds and margin collapsing are approximate. Re-evaluate at the M3 checkpoint (Taffy/Stylo vs. own engine).
- **Live JS** (`js::start`): a dedicated thread owns the QuickJS runtime. Load phase = scripts + DOMContentLoaded/load + the first virtual second of timers; then real-time timers. The UI sends `Cmd`s (events, field values, scroll, resize, layout boxes); after each burst the whole DOM is serialized (throttled to ~16/s) and, if changed, sent to a builder thread that fetches new images (URL cache) and builds a `Page` (latest wins, ~12/s). Node ids are the pre-order positions of the last serialization, so UI hits map straight to JS elements. The UI merges the new page with the user's field edits (`merge_fields`: user text wins unless a script changed the value). `run` (offline/tests) keeps the old virtual-time-only behaviour.
- **Fonts**: `Fonts` holds regular/bold/italic sets for sans and mono, lazily loaded coverage fallbacks (kept as bytes, outlined per glyph through `ttf-parser`; their cmap coverage is cached so a font that lacks a character is not read again) and per-page web fonts keyed by a global family id (TTF, OTF, WOFF, WOFF2); complex scripts are shaped per word with rustybuzz; bidi follows UAX #9 at word granularity.
- **Cookies**: sent/stored for the main navigation and for subresources of the same site only (a cheap third-party block). `Expires` is ignored; only `Max-Age` cookies persist.
- **Data dir**: `CHROME_RS_DATA`, else `%APPDATA%\chrome-rs`, else `~/.chrome-rs` (`cookies.tsv`, `localstorage.json`).
- **CSS values**: custom properties resolve at cascade time (`Style.vars`, inherited); `calc()` is a (percent, px) linear form (`Len::Mix`), so `calc(100% - 20px)` works in widths but not in places that only take px.
- **Pseudo-elements** are matched by the same rules (`Compound.pseudo`) and rendered as inline words; only text `content` is supported.
- **Security**: file: URLs are readable only from top-level navigation and file: pages; cross-origin script requests need CORS approval (no preflight, so non-simple requests are blocked); cookies are same-site only for subresources.
- **Paint keys and stacking contexts**: every `Rect` and `Placed` has `z: i64`, up to four base-32768 digits with the outermost stacking context most significant (`zwrap` shifts an inner context's digits down one place and puts the outer digit on top). In-flow content is 0, a positioned box digit `4*z+2`, `fixed` adds digit 16383. `layout_vh` sorts both lists by `z` (stable), `layers` returns each key with its ranges, and the painters draw a layer's rectangles first, then its text. The viewport height that `bottom` of a fixed box refers to is passed to `layout_vh` (relayout when it changes). `fixed` and `sticky` items carry `st` (scroll offset from which they pin, 0 for fixed) and `lim` (how far a sticky box may travel before its container ends; open values are negative while laying out and closed by `close_lims`); before painting, `main.rs` moves them by `min(max(0, scroll - st), lim)` in page coordinates, so hit-testing needs no special case.
- **Boxes**: `BoxSpec` carries vertical padding (`pt/pb`), alpha, gradient, shadow and `clip`. The margin gap before a boxed element excludes its padding, which the box paints itself. `overflow` clipping only applies with an exact `height`; it drops content below the box at layout time.
- **Scroll containers and box markers**: a region with `overflow: auto|scroll` and a fixed height keeps its height, tags everything inside it with a group id (`grp`) and emits an invisible marker `Rect` (`a = 0`, `scroll` = content height); any other box without a background emits a marker with the node only. `layout::groups` lists the containers; `main.rs` keeps a scroll offset per container node, moves the group's items by it and paints them with `gfx::set_clip` set to the box (the clip is honoured by every low-level painter), then draws the thumb. Nested containers: an item moves with its innermost container only.
- **Column flex and grid** are separate `Item`s laid out by recursive `block()` calls like row flex; grid supports column tracks and spans only.
- **Interaction state**: `css::set_state(hover, active, focus)` (process-wide atomics) feeds `:hover/:active/:focus`; a `Page` whose CSS uses them keeps `Restyle { dom, imgs, base }` and `main.rs` calls `render::restyled` when the state changes (items, images and page background are replaced; fields keep the user's edits).
- **Background images** are downloaded by `collect_images` under the key `usize::MAX - element id` (`render::background_urls` finds them with a cheap cascade pass), become a `BgPaint` on the `BoxSpec`, are resolved to a tile size and offset in `layout::resolve_pic` and painted by `gfx::draw_pic`.
- **Percentage heights** resolve in `render.rs` against `Ctx::cb_h`, the nearest ancestor's definite height (root: 720 px); heights from a percentage never clip.
- **Counters** live in the render context (`counters` stack plus `glob` for implicit ones); `content` values carry `\u{1}name\u{2}sep\u{3}` markers that `render.rs` resolves.
- **Text**: UAX #14 splits words into pieces (`render::split_cjk`); a piece that ends in a soft hyphen gets a visible hyphen only when the line breaks after it. `layout::bidi_reorder` runs UAX #9 on the words of a line and reorders them; fallback fonts are read once as bytes and outlined per glyph.
- **Windows build tip**: use a short `CARGO_TARGET_DIR` (long paths break the linker).

## Testing
- `cargo test` — unit tests next to the code (cascade, layout, cookies, JS, forms).
- `chrome-rs --corpus corpus [--bless] [--budget PCT]` — renders `corpus/*.html` offline to `corpus/out/*.png` and compares with `corpus/golden/*.png` (default budget 0.5 % differing pixels). Goldens depend on system fonts: bless once per machine.
- `chrome-rs --snap page.html out.png [width]` — one page to PNG.
- `chrome-rs --bench corpus [runs]` — median build / layout / paint milliseconds per page.
- `chrome-rs --save corpus/real corpus/urls.txt [--force]` — loads each URL once over the network (cookies go to a scratch directory) and archives every GET response under `corpus/real/NNN-name/` (`load::set_archive`: `index.tsv` plus one `.bin` per response). `--corpus` replays these archives through the normal loader, never touching the network, and compares with `corpus/real/golden/`. While replaying, `Math.random` is a fixed generator. The archives are git-ignored.
- `corpus/urls.txt` — the URLs of the real-page corpus.
- `CHROME_RS_TIMING=1` prints load-phase timings; `CHROME_RS_DEBUG=1` prints the first laid-out items.
