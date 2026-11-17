# chrome-rs

Small from-scratch browser in Rust. Own HTML parser, own CSS engine, JavaScript via embedded QuickJS, CPU rendering (no GPU).
~3 MB binary, ~30-40 MB RAM on typical pages.

```
cargo run --release -- [url or search words]
```

If the build fails with "path too long" on Windows, set `CARGO_TARGET_DIR` to a short path (e.g. `C:\t`).

## Browser features
- Tabs, **Ctrl+F** find in page (Enter / Shift+Enter / F3 next, Esc closes), mouse **text selection** with Ctrl+C / Ctrl+A / double-click word,
  **zoom** (Ctrl +/-/0, Ctrl+wheel), smooth scrolling and a draggable scrollbar, **Ctrl+D** bookmark (star in the address bar),
  `about:history` / `about:bookmarks` / `about:settings` (Ctrl+H / Ctrl+B), address-bar suggestions from bookmarks and history,
  **Ctrl+Shift+T** reopen a closed tab, **Ctrl+1..9** switch tab, previous session restored on start, **Ctrl+P** saves the page as a PDF
  (`prints` folder in the data dir), IME input. Settings (`home`, `search`) live in `settings.tsv` in the data dir.
- Fonts: real bold/italic faces, fallback fonts for symbols/CJK/Indic loaded on demand, `@font-face` (TTF/OTF/WOFF; not WOFF2),
  text shaping and word-level bidi for Arabic/Hebrew/Indic/Thai, CJK text wraps between characters.
- Live pages: scripts keep running after load (real-time timers, `requestAnimationFrame`), clicks/typing/focus/keys/scroll/resize reach
  page handlers (`addEventListener`, `onclick=`), the page is re-rendered when scripts change the DOM, `getBoundingClientRect`/`offset*`
  report real layout boxes, `location` assignments navigate, `<script type=module>` with imports, dynamic `import()` and top-level await.

## Keys / mouse
- Address bar: type a URL, a host (`rust-lang.org`) or search words (DuckDuckGo), Enter to go. Esc reverts, Ctrl+L selects.
- Click links. Wheel / arrows / PageUp / PageDown / Home / End scroll. Alt+Left/Right history. F5 / Ctrl+R reload.

## Pipeline
`load.rs` fetch (HTTPS, redirects) + external CSS/JS in parallel -> `dom.rs` parse -> `js.rs` run scripts/timers (4 s budget) ->
`render.rs` + `css.rs` cascade into styled words -> `main.rs` layout and paint.

## Works
- CSS: selectors (tag, class, id, attribute, descendant/child/sibling, first/last-child), specificity, `!important`, inline styles,
  `@media` (fixed 1000px viewport), `display`, `color`, `font-size`, `font-weight`, `text-align`, `text-decoration`, margins/padding
  (vertical gaps and left indent), page background, `list-style:none`, `visibility`/`opacity:0` hiding, `line-height`, `position: relative/absolute`, row flexbox (`flex`, grow/shrink/basis, wrap, gap, justify-content, align-items).
- JS: DOM (create/append/remove, innerHTML, attributes, classList, style, querySelector, events, DOMContentLoaded/load),
  timers in virtual time, promises, localStorage stub.

- Images (PNG, JPEG, GIF, WebP) fetched in parallel, downscaled on decode; block background colors (CSS and `bgcolor`).
- Pages in other encodings (windows-1251, etc.), JS `fetch`/XHR (GET only), Ctrl+C / Ctrl+V in the address bar.
- Layout: tables with real columns (colspan, widths, cellpadding/cellspacing, border, bgcolor, valign, nested tables), block boxes
  with margins, padding, `width`/`max-width`, `margin: 0 auto` centering, nested indentation and box backgrounds.
- Floats (`float` / `clear`: text wraps around them), CSS borders on blocks, `<hr>`, monospace fonts and preformatted blocks
  (`<pre>`, `<code>`, `white-space: pre`), `:not()` selectors, empty icon-only buttons are skipped.
- SVG: `<img src=*.svg>` and inline `<svg>` are rasterized once (resvg; shapes, paths, gradients, no `<text>`).
  Fixed-position overlays and "visually hidden" utility elements are not shown. `CHROME_RS_DEBUG=1` prints the first laid-out items.
- Tabs: Ctrl+T new, Ctrl+W close, Ctrl+Tab / Ctrl+Shift+Tab (or Ctrl+PageUp/Down) switch, click the strip, Ctrl+click a link opens it in a new tab.
- Forms: text/password fields with placeholders, textareas (wrapping, multi-line), dropdown selects, radio groups, checkboxes,
  buttons, hidden fields. Click to focus (caret goes where you click), Left/Right/Home/End/Delete/Backspace, Tab / Shift+Tab to move
  between fields, Ctrl+C / Ctrl+V, Enter or the button submits (GET and POST).
- `display:inline-block`, cookies (jar with domain/path/Secure/HttpOnly/Max-Age, persisted), persistent `localStorage`, `document.cookie`.
- Headless tools: `--snap in.html out.png`, `--pdf in.html out.pdf`, `--corpus DIR`, `--bench DIR`; `CHROME_RS_SELFTEST=1` exercises the UI paint paths and exits.
- CSS: `var()` custom properties, `calc()/min()/max()/clamp()`, `vw/vh/ch`, `:nth-child/-of-type/-last-child`, `:is()/:where()`, `:checked`, `:empty`,
  `::before/::after` with text `content`, `min-height`/`height` (as a minimum), `box-sizing`, `border-radius` (rounded fills and borders),
  `text-transform`, `white-space: nowrap`, `text-indent`, gradients as their first color, table `rowspan`.
- Clicking a `<label>` focuses its field or toggles its checkbox/radio.
- Schemes: `file://` (local pages and their CSS/JS/images), `data:`, `about:blank`, `view-source:`; `text/plain` shown preformatted.
- JS: `URL`, `URLSearchParams`, `FormData` (multipart), `Headers`/`Response`, `fetch`/XHR with POST/PUT/DELETE, `history.pushState`,
  `atob/btoa`, `TextEncoder/Decoder`, `Blob`, `performance.now`, `queueMicrotask`, `structuredClone`, `crypto`, `AbortController`, mouse/keyboard event classes.
  Cross-origin `fetch`/XHR follow a simplified CORS (simple requests only, response must allow the origin).
- Regression tooling: `--corpus`, `--snap`, `--bench` (see ARCHITECTURE.md).
- A panic while loading a page shows an error page instead of killing the browser.
- `CHROME_RS_TIMING=1` prints per-phase load timings to stderr.

## Not yet
SVG, file/date/range inputs, form validation, rowspan,
fixed positioning, flex column/stretch, grid, rounded corners, borders on inline elements, long `<pre>` lines are clipped (no horizontal scroll), pseudo-classes beyond the common ones, `box-shadow`, `opacity` below 1, background images,
WebSocket (always fails), Web Workers, MutationObserver, capture-phase events, hover/mouse-move events, WOFF2 fonts, text-selectable PDFs, JS-rendered SPAs, tabs, text selection in pages.
