# chrome-rs: roadmap and status

Status as of 2026-11-15 (M1, M2, M3 and M4 done). Legend: **[x]** done and covered by a test or a corpus page, **[~]** partly done (the limit is stated), **[ ]** not done.
Sizes are rough solo-developer estimates in weeks of focused work (with AI assistance), not promises.

## 0. What "finished" means

| Tier | Goal | Realistic? |
|------|------|------------|
| A. Reader/docs browser | Articles, docs, forums, search, wikis, simple forms | **Reached** for Latin/Cyrillic/CJK/RTL text pages |
| B. Mainstream web | Logins, shops, news with JS menus, no video | Partly: cookies, forms, live JS, flex, grid, fixed/sticky boxes, scroll containers, stacking contexts and modern CSS work on the real-page corpus; missing JS-rendered sites (frameworks), cache, HTTP/2, media, iframes, canvas |
| C. Modern web apps | Gmail/Docs/Maps/YouTube-class | No for a from-scratch engine |
| D. Chrome parity | Everything + extensions, sync, DRM | No |

**Target: tier B.** The engine decision is made (M3): keep the own engine; revisit only when a concrete feature (iframes, canvas, transitions, contenteditable) cannot be built on the current layout.

## 1. Snapshot

| | |
|---|---|
| Code | about 8,000 lines of Rust + 912 lines of JS prelude (`wc -l`), 12 Rust files |
| Tests | 113 unit tests (+1 ignored: TodoMVC apps) (cascade, layout, fonts, bidi, cookies, CORS and redirects against a local HTTP server, live-JS sessions, ES modules, PDF structure, UI logic) |
| Regression | `--corpus`: 15 hand-written offline pages and 164 archived real pages (of 168 URLs; `corpus/urls.txt`, re-saved 2026-11-15) rendered to PNG and compared with golden images (per-machine goldens; scripts use a fixed `Math.random` while replaying so shuffles do not flap) |
| Release binary | 8.3 MB; whole corpus (174 pages, replayed offline, release build) renders in about 87 s of CPU (was 72 s before the event, shadow DOM and `getComputedStyle` work; the machine was busy, not bisected) |
| Memory (release, Windows, 15 s after load, remeasured 2026-11-15) | example.com 78 MB working set (60 MB private); Hacker News 81 MB (63); MDN JavaScript guide 116 MB (99); Wikipedia Rust article 169 MB (153). Earlier numbers (static 54, HN 91, MDN 103) came from other builds and a 12 s wait: not comparable, the baseline grew with the larger prelude and the event system. A page with CJK text adds about 35 MB |
| Chrome comparison (2026-11-15, one run, Hacker News, 20 s after load, Windows, release build, clean Chrome profile) | chrome-rs: 1 process, 81 MB working set, 63 MB private. Chrome: 11 processes, 560 MB working set (sums shared pages more than once), 323 MB private. About 7x less working set and 5x less private memory on this one page, for a much smaller feature set. One page, one run: an indication, not a benchmark |
| Not measured | Chrome render time and reference screenshots; CPU load; Linux/macOS builds; memory over the whole real-page corpus (only four pages sampled) |
| Never done by hand | the GUI was not clicked through; UI behaviour is covered by pure-logic tests and the `CHROME_RS_SELFTEST` paint run |

## 2. Milestones

### M1. Fidelity of everyday pages: done
- [x] Inline formatting: `inline-block` (shrink-to-fit, baseline), `line-height`, `text-transform`, `white-space: nowrap`, `text-indent`, `<br>`, `letter-spacing`, `vertical-align: super/sub/<length>` on text
- [x] Positioning: `relative`, `absolute` (against the nearest positioned ancestor or the page; `top/right/bottom/left`, `inset`), **`fixed`** (own paint layer drawn at screen coordinates, `bottom`/`top` against the viewport height)
- [x] `sticky` pins to its `top` offset once the page scrolls past it and stops at the bottom of its container; `fixed` and `sticky` boxes are moved in page coordinates on every scroll, so hit-testing and clicks agree with what is painted
- [x] `z-index` and nested stacking contexts: every positioned box (and a relative one with a z-index) is a context, its content is ordered as a unit (a z-index:100 child of a z-index:5 box stays below a z-index:6 sibling); negative z-index paints below in-flow text, `fixed` boxes on top of everything. Contexts nest four deep, deeper levels share the fourth; z-index is clamped to +-4000. A positioned box with `z-index: auto` is treated as a context too
- [x] Flexbox: row (grow/shrink/basis, wrap, gap, justify-content, align-items/self, auto margins) and **column** (stacking with gap and margins, `align-items` across); column has no main-axis sizing (grow/justify need a container height)
- [x] Grid, columns only: `grid-template-columns` with `px`, `%`, `fr`, `repeat(n, ...)`, `repeat(auto-fill|auto-fit, minmax(N, 1fr))`, `gap`/`row-gap`/`column-gap`, `grid-column: span N`; rows are as tall as their tallest item. No explicit rows, areas, named lines, dense packing
- [x] Box model: `box-sizing`, `min-height`/`height`, `border-radius`, `rowspan`, `calc()/min()/max()/clamp()`, `var()`, `em/rem/vw/vh/ch`; **vertical padding is now painted inside the box background** (it used to be only a flow gap)
- [x] Selectors: `:nth-child/-last-child/-of-type`, `:first/last-of-type`, `:only-child`, `:empty`, `:is()/:where()`, `:checked`, `:disabled`, `::before/::after` with text `content`
- [x] Generated content and counters: `counter-reset/-increment`, `counter()` and `counters()` and `attr()` in `content`, `<ol>` numbering (`start`, `value`)
- [x] `box-shadow` (first layer: offset, blur, spread, color; drawn as stacked translucent rectangles; no `inset`)
- [x] `rgba()`/`#rrggbbaa` backgrounds are translucent; `linear-gradient()` is painted (angles, `to <side>`, stops with positions)
- [~] `opacity` below 1: text, box colors and images are blended with what is behind them; no group compositing (overlapping children of one faded box show through each other)
- [x] Scroll containers: `overflow: auto|scroll` with an explicit `height` keeps its size, its content is clipped to the box pixel-exactly and scrolls with the mouse wheel (the innermost container that can move takes the wheel, else the page), with a thumb drawn at the right edge. `overflow: hidden` with a height cuts the content. Not done: dragging the thumb, keyboard scrolling, horizontal scrolling, nested containers move only with their innermost offset, an `auto` container without a fixed height never scrolls, clicks on parts hidden by the clip still hit
- [x] Every box leaves an invisible geometry marker, so `:hover`, hit-testing and `getBoundingClientRect` see the box and its padding, not only its text
- [x] Backgrounds: `background-image: url()` (downloaded with the page's other images; `data:` URLs too) with `background-size` (auto, cover, contain, lengths, percentages), `background-position` (keywords, lengths, percentages), `background-repeat`, and the `background` shorthand; `radial-gradient()` (circle or ellipse, `at x y`, farthest-corner). Conic gradients fall back to the first color stop; only the first layer of a comma list is used; image corners are not rounded by `border-radius`
- [x] `:hover`, `:active`, `:focus`, `:focus-visible`, `:focus-within`: pages whose CSS uses them keep their DOM, and the page is restyled when the pointer moves to another element, the button is pressed or released, or focus moves. No transitions, no `cursor` styles; JS `mouseover` handlers are M4
- [x] Percentage `height` and `min-height` against a containing block that has a definite height (`html, body { height: 100% }` is the 720 px reference height); an auto-height parent ignores them. `html` and `body` are never clipped by `overflow`
- [~] `vertical-align: middle` on images; `top`/`bottom` keep the baseline, and inline boxes other than text and images ignore it
- [x] Exit criterion: a corpus of more than 100 pages. 15 hand-written offline pages plus 159 real pages (of the 168 URLs in `corpus/urls.txt`, the rest did not load: DNS, TLS handshake, timeouts), archived with `chrome-rs --save corpus/real corpus/urls.txt` and rendered offline through the full pipeline by `chrome-rs --corpus corpus`. The archives (261 MB) are not committed because they are third-party content, so the corpus can be rebuilt but not reproduced exactly (the sites change); goldens are per machine. A golden records what chrome-rs draws today, not what is correct: it catches regressions, it does not say a page looks right
- [x] Images and inline SVG take their size from CSS (`width`, `height`, `max-width`, `width: N%`) before the attributes; an unsized `<svg>` is at most 300 px wide. Found through the corpus, 61 of 159 pages changed
- [~] Images: `<picture><source>` (type and px media queries) and `srcset` pick the candidate a 1000 px screen at 1x would use (width descriptors: the smallest of at least 800 w), lazy `data-src`/`data-srcset` are used, `<svg><use href="#id">` copies its target from elsewhere in the page into the svg. Effect on the corpus is small (1904 image responses over 164 pages against 1774 over 159). Known gaps: pictures wider than 800 px are decoded downscaled, so a picture without any CSS or attribute size shows at that smaller size; `object-fit` and icon fonts are not handled

### M2. Text and fonts: done except weights, variable fonts, hyphenation and subpixel rendering
- [x] Real bold, italic and bold-italic faces (sans and mono families), lazily read from disk
- [x] Fallback fonts for symbols, CJK, Indic, Thai, loaded on demand and only when they cover the character; **outlines are read per glyph (`ttf-parser` + `ab_glyph_rasterizer`) instead of parsing the whole font**: a CJK page costs about 35 MB over a Latin page (debug build, Windows; it was 150-250 MB)
- [x] `@font-face` with TTF, OTF, WOFF 1 and **WOFF 2** (`woff2-patched`: brotli and the glyf/loca transform); relative `url()` in external stylesheets is made absolute; `font-display` needs no support because fonts are fetched before the first paint
- [x] Text shaping with rustybuzz for Hebrew, Arabic, Indic, Thai (ligatures, joining, mark placement)
- [x] Bidi by UAX #9 (`unicode-bidi`): paragraph direction from the first strong character, embedding levels per word, visual reordering of the words of each line, numbers inside RTL text keep their order, lone brackets are mirrored in RTL runs. Granularity is the word (mixed-script words are only reordered by the shaper)
- [x] Line breaking by UAX #14 (`unicode-linebreak`): after hyphens and slashes (long URLs wrap), between CJK characters with the kinsoku rules (no line starts with a closing mark or ends with an opening one), after soft hyphens (a hyphen is drawn only where the line breaks)
- [ ] Variable fonts (a variable WOFF2/TTF loads at its default instance, bold is faked) and `font-weight` values 100-900 (only regular/bold exist)
- [ ] Automatic hyphenation (`hyphens: auto` needs dictionaries); only soft hyphens work
- [ ] Sub-pixel antialiasing, hinting at fractional scales

### M3. Architecture checkpoint: decided, keep the own engine
Decision (2026-11-14): no switch to Servo or Blitz. Keep the item-stream layout with the box markers, scroll groups and paint keys added in M1; do not rewrite it into a box tree now.

Data behind it:
- M1 and M2 were done on the existing layout. What looked like architecture limits (stacking contexts, scroll containers, sticky limits, hover, percentage heights, background images) turned out to need markers and tags on the output, not a new tree. The one rewrite I proposed (a box tree) was not needed to deliver them.
- The 159-page real corpus renders, with no crashes after the stack fix, and a contact-sheet review of 80 of the pages shows text, tables, floats, flex and grid, and Latin/Cyrillic/CJK/Arabic/Hebrew/Indic/Thai pages all laid out recognisably. The visible defects fall in three classes:
  1. images and inline SVG drawn at their natural size because CSS `width`/`height`/`max-width` on `<img>` and `<svg>` was ignored (rust-lang book icons, Flask, Deno, PHP, Python, IETF, sqlite sponsors, GitHub): a bug in `render.rs`, not in the layout model. Fixed afterwards: CSS sizes beat the attributes, one given dimension fixes the other by the picture's proportions, `max-width` and `width: N%` work, an unsized inline `<svg>` is at most 300 px wide; 61 of the 159 real pages changed and were re-blessed;
  2. pages that need JavaScript to show anything (crates.io, Stack Overflow, Medium, Reddit, IMDb, NYTimes, OpenStreetMap): the M4 scope (DOM and Web API coverage);
  3. fine details (icon fonts, `<svg>` `<use>`, form control looks).
- Cost of the alternatives is unchanged: Blitz would cost the binary size (7.8 MB) and memory (54 MB static page) that are this project's selling point and a rewrite of the QuickJS bridge; Servo is a different project. Neither was prototyped, so those costs are estimates.

Revisit when something needs a real box tree: iframes with their own documents, canvas, CSS transitions and animations, text editing in `contenteditable`, or when the corpus shows layout (not sizing) defects the item stream cannot express. The trigger is a concrete feature that cannot be built on the markers, not an estimate overrun.

### M4. Live JavaScript and DOM: done (one page at a time)
- [x] A session thread keeps QuickJS alive after load; first virtual second of timers at load, then real-time timers and `requestAnimationFrame`
- [x] UI events reach page handlers (`click`, `mousedown/up`, `keydown/up`, `input`, `change`, `focus`, `blur`, `submit`, `scroll`, `resize`); `addEventListener`, `onclick=` attributes and `el.onx = fn`; `preventDefault` / `return false` cancel link following and form submission
- [x] Typed text, checkbox, radio and select state is mirrored into the page DOM; the UI merges edits with script changes when the page re-renders
- [x] DOM changes re-render the page (throttled; images cached by URL); `location` assignment and `location.href` navigate; `history.pushState`, `popstate`
- [x] Layout APIs from real boxes: `getBoundingClientRect`, `offset*`, `client*`, `scrollTop`; `IntersectionObserver`/`ResizeObserver` report everything visible
- [x] Web APIs: `fetch`/XHR (all methods, FormData multipart, Headers, Response), `URL`, `URLSearchParams`, `Blob`, `TextEncoder/Decoder`, `atob/btoa`, `AbortController`, `performance`, `crypto`, `structuredClone`, `queueMicrotask`, persistent `localStorage`, `document.cookie`
- [x] ES modules: `<script type=module>`, static and dynamic `import()`, top-level await
- [x] `document.implementation` (`createHTMLDocument`, which jQuery needs at load) and the `HTML*Element` globals: `jQuery is not defined` fell from 26 to 7 corpus pages, usa.gov and gov.uk now run their banner and menu scripts
- [x] **Page scripts run in sloppy mode** (rquickjs evals in strict mode by default, so `OSM = {}` without `var` threw `OSM is not defined`): this one switch removed the `RLQ`/`RLCONF` (Wikipedia), `sinaads`, `jsLoader`, `_N_E` and OpenStreetMap errors, and python.org and wikipedia.org now run their menus and portal scripts instead of showing the no-script fallback
- [x] `document.currentScript` (the running script's `src`): Turbopack/Next.js bundles (nodejs.org, tailwindcss.com) no longer throw `chunk path empty but not in a worker`
- [x] `<script type="importmap">` (exact and `prefix/` entries, no `scopes`), so bare module specifiers like `react` resolve; `el.attributes` answers `attributes.name` and `getNamedItem` (jQuery 1.x read `attributes[n].expando` and died, taking `$`/`jQuery` down on about 16 pages); globals `File`, `NodeList`, `HTMLCollection`, `ShadowRoot`, `SVGElement`, `Storage`, `screen`; a small English-only `Intl` (DateTimeFormat, NumberFormat, Collator, PluralRules, RelativeTimeFormat, ListFormat, Segmenter), `Comment`, `Range`, `TreeWalker`, `MessageChannel`, `FileReader`, `Request`, `document.styleSheets/scripts/forms/links/images/fonts`, `performance.timing`, `navigator.*`, `HTML*Element` that test the tag, and about 25 `Element` methods (`insertAdjacentElement`, `replaceWith`, `getAttributeNames`...)
- [x] **Custom Elements**: `customElements.define/get/whenDefined/upgrade`, `class X extends HTMLElement`, upgrade of elements already in the page, `connectedCallback`/`disconnectedCallback`/`attributeChangedCallback`. Not done: customized built-ins
- [x] `import.meta.url` (webpack/Vite derive their public path from it), up to 400 imported modules per page (was 60), `HTMLScriptElement.noModule` (GOV.UK Frontend), `document.adoptedStyleSheets`, `<template>.content`, `form.elements`, `control.form`, `a.pathname/search/hash/host/...`, absolute `a.href` and `script/img.src`, `Text.nodeValue` setter, `meta.content`. A prelude that fails to load now says so under `CHROME_RS_JSERR=1` (it used to switch all JS off silently)
- [x] **Comment nodes** are kept in the DOM for scripts (`<!--[-->` anchors are what Vue and React hydration walk; Font Awesome SVGs carry one too) and stripped before layout and before an inline `<svg>` is written out; root-level comments of a document are dropped. `<select>`: `options`, `selectedIndex`, `selectedOptions`, option `index/text/defaultSelected`: react.dev and vuejs.org now hydrate and render in full
- [x] `CHROME_RS_JSERR=1` also prints `console.error`/`warn` and exceptions thrown inside timers (`JSLOG`), which pages such as Vue and Next.js use to report their own failures; both were swallowed before prints the script exceptions that were swallowed before; use it with `--corpus` to find what breaks
- [ ] Capture phase, hover/mouse-move/pointer events, `MutationObserver` (stub), Web Workers, WebSocket (always fails), `IndexedDB`, `sessionStorage` persistence, `matchMedia` (always false)
- [ ] Incremental restyle/relayout: every update rebuilds the whole page and re-serializes the whole DOM (fine for ordinary pages, slow for 10k+ nodes)
- [x] `getComputedStyle` runs a small cascade over the page's `<style>` elements and the inline style (UA display defaults, specificity, `!important`, inheritance of color/font/visibility...). Declared values come back as written (no unit conversion, no `var()`/`calc()`, `@media` blocks skipped, no pseudo-elements); width/height come from the last layout box. jQuery 2 `show()`/`toggle()` depend on it. It costs CPU: the rule scan is redone every 32 calls and rules are pre-filtered by class/id/tag. `innerText` is still `textContent`
- [~] Blank corpus pages, investigated: crates.io (404), Stack Overflow, NYTimes, Medium, blender.org (403) and IMDb (202) were archived as server errors or bot checks, so they are **not** JS defects; a re-save with a browser User-Agent or from another network may fix them. Left after the fixes below (`CHROME_RS_JSERR=1 --corpus`: 47 error lines over 159 pages, was 300+): `Error loading module .../react-*.js` on GitHub (the import map resolves, but the archive has no copy of the file: re-save with `--save` to check), `not a function` (3, mixed causes), `charAt`/`type`/`replace` of undefined, `ReadableStream` and friends are inert stubs, tailwindcss.com and asahi.com hit the JS CPU budget (`interrupted`; the budget is wall-clock, so on a loaded machine their corpus result flips: a golden mismatch there is not necessarily a regression), Swagger UI on httpbin.org renders its list but not `SchemesContainer`; tools.ietf.org/html/rfc2616 hydrates into an `Error 500` page (a Nuxt route chunk is missing from its archive even after a re-save, cause not found: its golden records that). **Traps found while adding APIs** (an API that exists but is half-done is worse than a missing one, because pages feature-detect it): `DOMParser` present breaks DOMPurify (Swagger UI), so it stays out until it is complete. `PromiseRejectionEvent` and `namespaceURI` looked harmful too, but the blank react.dev and vuejs.org they produced came from two other causes, now fixed: a stale archive (the corpus copy lacked a chunk that hydration now asks for; re-saved) and `select.options` missing (React's `<select>` code). `instanceof HTMLIFrameElement` must test the tag. Open: archive.org, naver.com, Reddit not looked at
- [x] **Events follow the DOM model**: capture, target and bubble phases (`addEventListener(t, f, {capture, once})`, `eventPhase`, `composedPath`, `stopImmediatePropagation`); keyboard events carry `keyCode`/`which`/`code` derived from `key`; `keypress` is sent for Enter and characters; Enter or blur in a changed field fires one `change` (the field's baseline is updated so a script that clears it is not overwritten); `onclick`, `oninput`... exist as null properties (React tests `'oninput' in document`; touch handlers are deliberately absent, python.org's carousel switches to a touch layout if `ontouchstart` exists)
- [x] Parser fixes found through the apps: whitespace-only text at the root of a fragment is kept (Svelte templates `<!> <main>` walk siblings); a stray quote after a value (`data-id="x"">`, produced by Handlebars) no longer swallows the following tags (apache.org and schneier.com now render fully); `innerHTML` of `<script>`/`<style>` is raw text (Backbone templates); `iframe.contentWindow/contentDocument` give an empty document, `onhashchange` exists (jQuery 2 and Backbone 1 write into an iframe)
- [x] **Shadow DOM, flattened**: `attachShadow` roots are real trees (`innerHTML`, `append`, `mode`, `host`, events bubble through the host); when the page is serialized a host shows its shadow tree instead of its light children, a `<slot>` shows the nodes assigned to it (by `slot=` name, fallback content otherwise; `assignedNodes()`), and constructable stylesheets adopted by a root (`adoptedStyleSheets` with `replaceSync`) become `<style>` elements. Shortcuts: shadow styles apply to the whole page (no scoping), `:host` and `::slotted` do not match, events are not retargeted, `cloneNode`/`importNode` construct defined custom elements at once (template content stays inert). Also: `<?x>` parses as a comment (Lit's part markers), `TreeWalker` walks the live tree from `currentNode`, `hasAttributes()`
- [x] **Exit criterion met**: ten TodoMVC apps were archived (`corpus-m4/`, not committed) and driven through the live session (type, Enter, read the list back; `cargo test --release archived_todomvc -- --ignored`): React, Vue, Preact, Svelte 5, Angular, ES6, Backbone, jQuery, Lit and web-components all accept a todo and render it. The test drives events directly, not through the GUI. Svelte needed plain elements to sit one prototype below `Element.prototype` (it collects setters such as `hidden` from that part of the chain; without it `hidden=false` became `setAttribute('hidden', false)` and the list stayed hidden)

### M5. Networking and web platform: partly done
- [x] Cookie jar (domain, path, Secure, HttpOnly, Max-Age; persisted), redirects handled hop by hop, same-site cookies only for subresources
- [x] Schemes: `file://` (readable only by top-level navigation and file pages), `data:`, `about:blank`, `view-source:`, `text/plain` shown preformatted
- [x] Charset decoding, gzip via the HTTP client
- [ ] `Expires` is ignored (only `Max-Age` persists), no `SameSite`, no public-suffix check
- [ ] HTTP cache (ETag, Cache-Control), HTTP/2 and HTTP/3, brotli/zstd, proxy settings, retry policy
- [ ] HSTS, certificate error page, mixed-content blocking, HTTPS-only mode
- [ ] `multipart/form-data` file uploads, file/date/range/color inputs, form validation, autocomplete, password manager
- [ ] Downloads manager, content-type sniffing, in-browser PDF viewing
- [ ] `<video>`/`<audio>`, `<canvas>`, `<iframe>`, `blob:` URLs

### M6. Browser product features: mostly done
- [x] Find in page (Ctrl+F, Enter/Shift+Enter/F3, highlights, centered scroll)
- [x] Text selection with the mouse, double click for a word, Ctrl+A, Ctrl+C
- [x] Zoom (Ctrl +/-/0, Ctrl+wheel), smooth scrolling, draggable scrollbar
- [x] Bookmarks (Ctrl+D, star indicator), history, `about:history`, `about:bookmarks`, `about:settings`, address-bar suggestions
- [x] Reopen closed tab (Ctrl+Shift+T), Ctrl+1..9, session restore on start, home page and search engine in `settings.tsv`
- [x] IME commit text, `<label>` click focuses or toggles its control
- [x] Print to PDF (Ctrl+P): the page is rendered and cut into A4 sheets as images (not selectable text)
- [ ] Context menu, favicons, tab reorder/pin/duplicate, multiple windows, light/dark theme and `prefers-color-scheme`
- [ ] Developer tools (DOM tree, computed style, console, network)
- [ ] Drag and drop, touch/trackpad gestures, kinetic scrolling, caret browsing
- [ ] A real settings UI (settings are a text file), real print dialog and text PDFs
- [ ] Exit criterion not met: not used as a daily browser

### M7. Performance and memory: not started
- [ ] Incremental layout and paint (damage rectangles), layer caching, GPU backend (wgpu/Vello) with the CPU path as fallback
- [ ] Lazy image decode, viewport-only painting, per-tab memory budgets, tab discarding
- [ ] Startup budget, selector matching profiling, JS snapshots
- [~] Tooling exists: `--bench` prints median build/layout/paint milliseconds per corpus page (about 3 ms build, under 1 ms layout on the corpus in release); `CHROME_RS_TIMING=1` prints load phases
- Known costs: CJK fonts (see M2), whole-DOM re-serialization on live pages, every live update rebuilds the page and clones cached images

### M8. Security and isolation: first steps only
- [x] `file:` isolation, cookies scoped to the site for subresources, simplified CORS for script requests (simple requests only, no preflight, response must allow the origin)
- [x] Parser and runtime limits: depth cap, response size limits, JS CPU budget per burst, memory limit
- [ ] Same-origin policy for the DOM (there is only one document, no iframes yet), CSP, `Referrer-Policy`, sandboxed iframes, `X-Frame-Options`
- [ ] Process isolation and OS sandbox (renderer, network and UI share one process)
- [ ] Fuzzing (HTML, CSS, URL, image, SVG, fonts, WOFF), `cargo-deny`/`cargo-audit`
- [ ] Permission prompts, blocklist, signed auto-update
- [ ] Exit criterion not met: no external review of the renderer boundary

### M9. Accessibility and internationalisation: not started
- [ ] Accessibility tree and platform bridges (`accesskit`), keyboard-only navigation and focus ring, `aria-*`, high contrast, reduced motion
- [ ] UI translation (Russian first), locale-aware `Intl`, spell-check, right-to-left UI chrome

### M10. Compatibility, QA and release: not started
- [~] Unit tests and the corpus exist (174 pages); no web-platform-tests subset, no reference screenshots from another browser to compare with, no fuzzers
- [ ] CI on Windows/Linux/macOS, reproducible builds, crash reporter
- [ ] Installers (MSI/MSIX, AppImage/deb, dmg), code signing, update channels
- [ ] User manual and contributor guide (README and ARCHITECTURE exist and are current)
- [ ] Linux and macOS were never built or run: font paths for them are written but untested

## 3. Order of work from here (tier B, one developer)

| Order | Work | Why first |
|-------|------|-----------|
| 1 | The blank JS pages (crates.io, Stack Overflow, Medium, Reddit, IMDb, NYTimes, OpenStreetMap; some are server errors, see M4) | Last open defects of the JS class |
| 2 | Memory over the whole corpus and render time against Chrome on a sample (HN memory re-measured 2026-11-15) | Only four pages were sampled |
| 3 | Corpus follow-ups: icon fonts, form control looks, `object-fit`, decode large images at full size; reference screenshots from another browser for the worst pages | The remaining visible gaps; goldens alone cannot say what is wrong |
| 4 | HTTP cache, HTTP/2, brotli, file uploads, form validation, `<iframe>`, `<canvas>` (iframes and canvas are the M3 revisit triggers) | Logins, shops and embedded content |
| 5 | CI that runs `cargo test` and `--corpus` on every change (needs the archives somewhere: a release asset or a cache) | The corpus only helps if it runs |
| 6 | Security: CSP, DOM same-origin with iframes, fuzzing, renderer process split | Must land before any public release |
| 7 | Accessibility, Linux/macOS builds, installers | Release work |

## 4. Biggest risks

1. **Scope explosion.** Every CSS property is endless. Mitigation: the corpus drives the backlog; build only what blocks listed pages.
2. **Own engine vs. reuse.** Decided at M3 (keep). The risk that remains is a feature that really needs a box tree (iframes, canvas, transitions); it is revisited then, with the corpus as evidence.
3. **Memory.** Eager font parsing and whole-page rebuilds on live pages are the current hot spots; process isolation (M8) will add to it.
4. **JavaScript compatibility.** QuickJS is complete for the language but slow; live pages re-serialize the DOM, so large SPAs will lag.
5. **Security is not optional.** One process, no sandbox, no CSP yet: do not browse untrusted sites with it as a daily browser.
6. **Unverified GUI.** Interactive behaviour (drags, shortcuts, focus flows) was tested through logic and a headless paint run, not by hand; expect bugs in the first manual sessions.
7. **Bus factor.** `ARCHITECTURE.md` documents the data flow and decisions; keep tests green on every change.
8. **Corpus is a regression net, not a correctness check.** Goldens record chrome-rs's own output; the archives are not committed, so a fresh `--save` gives different pages. Mitigation: reference screenshots from another browser for the worst pages, and archives kept as a release asset.
