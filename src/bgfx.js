// Animated ASCII backgrounds, shared by Settings, the artwork picker and
// Fix Match (webwin.rs puts this file in place of each page's BGFX marker).
//
// One canvas behind the page at 30%. A pattern fills a character grid
// (glyph + brightness 0..3) for tick t; it's drawn ten times a second, one
// fillText per row per brightness. Nothing runs while the window is hidden
// or the setting is off, and Reduce Motion gets a single still frame.
const Bgfx = (() => {
const hash = (x, y, z = 0) => {
  let h = (x * 374761393 + y * 668265263 + z * 2147483647) | 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  return ((h ^ (h >>> 16)) >>> 0) / 4294967295;
};
const clamp01 = v => v < 0 ? 0 : v > 1 ? 1 : v;
const wrapN = (v, m) => ((v % m) + m) % m;
const FONT = '12px ui-monospace, "SF Mono", "Cascadia Mono", Consolas, monospace';
const ALPHA = [0.5, 0.7, 0.88, 1];
const STILL_T = 64; // the frame shown under Reduce Motion
const reduce = matchMedia("(prefers-reduced-motion: reduce)");
const s = { cols: 0, rows: 0, vcols: 0, vrows: 0, cw: 7.2, ch: 14, glyph: [], level: null };
let cv = null, g = null, pattern = null, on = false, timer = 0, t = 0;

function size() {
  const dpr = devicePixelRatio || 1, w = innerWidth, h = innerHeight;
  cv.width = Math.round(w * dpr); cv.height = Math.round(h * dpr);
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.font = FONT; g.textBaseline = "top";
  s.cw = g.measureText("M").width || 7.2; s.ch = 14;
  s.cols = Math.ceil(w / s.cw); s.rows = Math.ceil(h / s.ch);
  s.vcols = Math.floor(w / s.cw); s.vrows = Math.floor(h / s.ch); // fully visible cells
  s.glyph = new Array(s.cols * s.rows).fill(" ");
  s.level = new Uint8Array(s.cols * s.rows);
  pattern && pattern.init && pattern.init(s);
}

function draw() {
  s.glyph.fill(" "); s.level.fill(0);
  pattern.frame(s, t);
  g.clearRect(0, 0, innerWidth, innerHeight);
  g.fillStyle = "#eef0ff";
  const line = new Array(s.cols);
  for (let lv = 0; lv < 4; lv++) {
    g.globalAlpha = ALPHA[lv];
    for (let r = 0; r < s.rows; r++) {
      let any = false;
      for (let c = 0; c < s.cols; c++) {
        const i = r * s.cols + c;
        const hit = s.level[i] === lv && s.glyph[i] !== " ";
        line[c] = hit ? s.glyph[i] : " ";
        any = any || hit;
      }
      if (any) g.fillText(line.join(""), 0, r * s.ch);
    }
  }
  g.globalAlpha = 1;
}

function tick() { timer = 0; t++; draw(); schedule(); }
function schedule() {
  clearTimeout(timer); timer = 0;
  if (on && pattern && !document.hidden && !reduce.matches) timer = setTimeout(tick, 100);
}
function refresh() {
  if (!cv) return;
  cv.hidden = !on;
  if (!on || !pattern) { clearTimeout(timer); timer = 0; return; }
  if (reduce.matches) t = STILL_T;
  draw(); schedule();
}
function mount() {
  if (cv) return;
  const style = document.createElement("style");
  style.textContent = "#bgfx { position: fixed; inset: 0; width: 100vw; height: 100vh; z-index: 0; pointer-events: none; opacity: 0.3; }"
    + " #bgfx[hidden] { display: none; } .app { position: relative; z-index: 1; }"
    // Text drawn straight on the pattern gets a soft halo of the page colour, so
    // the characters behind it never run into the letters.
    + " body:has(#bgfx:not([hidden])) :is(h1, h2, .lead, .plabel, .dcap, .result, .label, .title, .subtitle, .hint, .urlnote, .empty, .now)"
    + " { text-shadow: 0 0 2px var(--color-paper), 0 0 6px var(--color-paper), 0 0 12px var(--color-paper); }";
  document.head.appendChild(style);
  cv = document.createElement("canvas");
  cv.id = "bgfx"; cv.hidden = true; cv.setAttribute("aria-hidden", "true");
  document.body.prepend(cv);
  g = cv.getContext("2d");
  size();
  addEventListener("resize", () => { size(); refresh(); });
  document.addEventListener("visibilitychange", schedule);
  reduce.addEventListener("change", refresh);
}

/* Images: TMDB serves artwork with CORS, so the canvas can read it. Each URL
 * is tried in turn; one that can't be read is skipped. Downscaled to 480 wide.
 * TMDB only sends the CORS header when asked, so a copy the page already has
 * (the preview, the tiles) can't be reused: these requests get their own
 * cache key, with the plain URL as a fallback. */
function loadImage(urls, done) {
  urls = [...new Set(urls.filter(u => typeof u === "string" && /^https:\/\//.test(u)))]
    .flatMap(u => [u + (u.includes("?") ? "&" : "?") + "bgfx", u]);
  const next = () => {
    const url = urls.shift();
    if (!url) return;
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => {
      try {
        const k = Math.min(1, 480 / img.naturalWidth);
        const w = Math.max(1, Math.round(img.naturalWidth * k)), h = Math.max(1, Math.round(img.naturalHeight * k));
        const c = document.createElement("canvas"); c.width = w; c.height = h;
        const x = c.getContext("2d"); x.drawImage(img, 0, 0, w, h);
        done(x.getImageData(0, 0, w, h), url);
      } catch { next(); }
    };
    img.onerror = next;
    img.src = url;
  };
  next();
}

function paintStill(w, h) {
  const c = document.createElement("canvas"); c.width = w; c.height = h;
  const x = c.getContext("2d");
  const sky = x.createLinearGradient(0, 0, 0, h * 0.62);
  sky.addColorStop(0, "#05060a"); sky.addColorStop(1, "#4a4f63");
  x.fillStyle = sky; x.fillRect(0, 0, w, h);
  const moon = x.createRadialGradient(w * 0.72, h * 0.26, 0, w * 0.72, h * 0.26, h * 0.3);
  moon.addColorStop(0, "#ffffff"); moon.addColorStop(0.22, "#e8e8e8"); moon.addColorStop(0.24, "#6a6f80"); moon.addColorStop(1, "rgba(60,64,80,0)");
  x.fillStyle = moon; x.fillRect(0, 0, w, h);
  x.fillStyle = "#15171f"; // far hills
  x.beginPath(); x.moveTo(0, h * 0.6);
  for (let i = 0; i <= 12; i++) x.lineTo(w * i / 12, h * (0.5 + 0.06 * Math.sin(i * 1.7) + 0.03 * Math.cos(i * 3.1)));
  x.lineTo(w, h); x.lineTo(0, h); x.fill();
  x.fillStyle = "#0a0b10"; x.fillRect(0, h * 0.64, w, h); // ground
  x.fillStyle = "#565b6b"; // road to a vanishing point
  x.beginPath(); x.moveTo(w * 0.38, h); x.lineTo(w * 0.53, h * 0.64); x.lineTo(w * 0.56, h * 0.64); x.lineTo(w * 0.86, h); x.fill();
  x.fillStyle = "#c9c9c9";
  for (let i = 0; i < 7; i++) { const k = i / 7, y = h * (0.66 + 0.34 * k * k); x.fillRect(w * (0.545 + 0.07 * k * k) - 1, y, 2 + 6 * k, 3 + 14 * k * k); }
  x.fillStyle = "#020203"; // a lone figure and a lamp post
  x.fillRect(w * 0.3, h * 0.38, w * 0.008, h * 0.4);
  x.fillRect(w * 0.3, h * 0.38, w * 0.05, h * 0.01);
  x.beginPath(); x.ellipse(w * 0.62, h * 0.76, w * 0.012, h * 0.03, 0, 0, 7); x.fill();
  x.fillRect(w * 0.607, h * 0.78, w * 0.026, h * 0.12);
  const lamp = x.createRadialGradient(w * 0.348, h * 0.4, 0, w * 0.348, h * 0.4, h * 0.22);
  lamp.addColorStop(0, "rgba(255,255,255,0.75)"); lamp.addColorStop(1, "rgba(255,255,255,0)");
  x.fillStyle = lamp; x.fillRect(0, 0, w, h);
  return x.getImageData(0, 0, w, h);
}

/* ---------------- Artwork as characters ---------------- */
// Settings' Still, and the picker's darkroom. A new image "develops": each
// cell holds noise until its own moment in the first ~1.4s, then settles.
const RAMP = " .:-=+*#%@";
const NOISE = "!#$%&*+-:;<=>?@^~";
const DEVELOP = 14;
function artwork() {
  return {
    img: null, lum: null, since: -1e9, s: null,
    setImage(data) {
      this.img = data; this.since = t;
      if (this.s) this.init(this.s);
      if (pattern === this) refresh();
    },
    init(s) {
      this.s = s;
      this.img = this.img || paintStill(480, 270);
      // Cover the window, sampling at each cell's centre (cells are tall, so this keeps the aspect).
      const { width: iw, height: ih, data } = this.img;
      const W = s.cols * s.cw, H = s.rows * s.ch, k = Math.max(W / iw, H / ih);
      const ox = (W - iw * k) / 2, oy = (H - ih * k) / 2;
      this.lum = new Float32Array(s.cols * s.rows);
      for (let r = 0; r < s.rows; r++) for (let c = 0; c < s.cols; c++) {
        const px = Math.min(iw - 1, Math.max(0, Math.floor(((c + 0.5) * s.cw - ox) / k)));
        const py = Math.min(ih - 1, Math.max(0, Math.floor(((r + 0.5) * s.ch - oy) / k)));
        const p = (py * iw + px) * 4;
        this.lum[r * s.cols + c] = (0.2126 * data[p] + 0.7152 * data[p + 1] + 0.0722 * data[p + 2]) / 255;
      }
    },
    frame(s, t) {
      // Shimmer: a soft band of light crossing on the diagonal every ~14s,
      // and a few cells nudging a step up or down the ramp each tick.
      const span = s.cols + s.rows * 2, band = ((t / 140) % 1) * (span + 40) - 20;
      const age = reduce.matches ? DEVELOP : t - this.since;
      for (let r = 0; r < s.rows; r++) for (let c = 0; c < s.cols; c++) {
        const i = r * s.cols + c;
        if (age < DEVELOP && age < hash(c, r, 77) * DEVELOP) {
          if (hash(c, r, t) < 0.35) { s.glyph[i] = NOISE[Math.floor(hash(c, r, t + 1) * NOISE.length)]; s.level[i] = 0; }
          continue;
        }
        let v = this.lum[i];
        const d = (c + r * 2) - band;
        v += 0.22 * Math.exp(-(d * d) / 180);
        const n = hash(c, r, t >> 1);
        if (n < 0.04) v += 0.11; else if (n > 0.96) v -= 0.11;
        v = clamp01(v);
        const k = Math.round(v * (RAMP.length - 1));
        s.glyph[i] = RAMP[k];
        s.level[i] = Math.min(3, k >> 1);
      }
    },
  };
}

/* Shared: write one cell, keeping a brighter glyph that's already there. */
const putter = s => (c, r, ch, lv) => {
  c = Math.round(c); r = Math.round(r);
  if (ch === " " || c < 0 || r < 0 || c >= s.cols || r >= s.rows) return;
  const i = r * s.cols + c;
  if (s.glyph[i] !== " " && s.level[i] > lv) return;
  s.glyph[i] = ch; s.level[i] = lv;
};

/* ---------------- Aquarium: fish, bubbles, swaying weed ---------------- */
// Every position is a function of t, so the Reduce Motion frame is just t = 64.
const FISH_R = [["><(((º>"], ["><>"], ["><((('>"], ["  __", "\\/ o\\", "/\\__/"], [">=>"], ["   ,", "><'__)>"]];
const FISH_L = [["<º)))><"], ["<><"], ["<')))><"], [" __", "/o \\/", "\\__/\\"], ["<=<"], ["  ,", "<(__'><"]];
const aquarium = {
  frame(s, t) {
    const put = putter(s), floor = s.vrows - 1;
    // Sand.
    for (let c = 0; c < s.cols; c++) put(c, floor, hash(c, 3) < 0.3 ? "." : hash(c, 4) < 0.5 ? "~" : "_", 0);
    // Weed: a stalk every ~11 columns, each segment leaning with a slow wave.
    for (let k = 0, c0 = 3; c0 < s.cols; k++, c0 += 7 + Math.floor(hash(k, 5) * 9)) {
      const h = 3 + Math.floor(hash(k, 6) * 7);
      for (let seg = 1; seg <= h; seg++) {
        const lean = Math.sin(t * 0.06 + k * 1.3 - seg * 0.5) * (seg / h) * 1.6;
        put(c0 + lean, floor - seg, lean >= 0 ? ")" : "(", 1);
      }
    }
    // Bubbles rising in a few columns, wobbling as they go.
    const streams = Math.max(3, Math.round(s.cols / 30));
    for (let k = 0; k < streams; k++) {
      const c0 = 4 + hash(k, 7) * (s.cols - 8);
      for (let j = 0; j < 4; j++) {
        const y = floor - 1 - wrapN(t * 0.35 + j * (s.rows / 4) + hash(k, 8) * s.rows, s.rows);
        const g = y < s.rows * 0.3 ? "O" : y < s.rows * 0.65 ? "o" : ".";
        put(c0 + Math.sin(y * 0.5 + k) * 0.8, y, g, 2);
      }
    }
    // Fish.
    const n = Math.max(7, Math.round(s.cols * s.rows / 650));
    for (let i = 0; i < n; i++) {
      const right = hash(i, 11) < 0.5, kind = Math.floor(hash(i, 12) * FISH_R.length);
      const sprite = (right ? FISH_R : FISH_L)[kind];
      const speed = 0.12 + hash(i, 13) * 0.35, span = s.cols + 24;
      const run = wrapN(hash(i, 14) * span + speed * t, span);
      const x = right ? run - 12 : s.cols + 12 - run;
      const y = 2 + hash(i, 15) * (floor - 6 - sprite.length) + Math.sin(t * 0.04 + i * 2) * 0.8;
      sprite.forEach((line, dy) => { for (let dx = 0; dx < line.length; dx++) put(x + dx, y + dy, line[dx], 3); });
    }
  },
};

/* ---------------- Night sky: twinkling stars, a shooting star now and then ---------------- */
const sky = {
  frame(s, t) {
    const put = putter(s), drift = Math.floor(t * 0.02);
    for (let r = 0; r < s.rows; r++) for (let c = 0; c < s.cols; c++) {
      const wc = c + drift; // the field drifts left, very slowly
      const h = hash(wc, r, 5);
      if (h > 0.03) continue;
      // Each star twinkles on its own slow clock.
      const tw = hash(wc, r, Math.floor(t / 5 + hash(wc, r, 6) * 9));
      if (h < 0.003) put(c, r, tw < 0.2 ? "+" : "*", 3);
      else put(c, r, tw < 0.15 ? "+" : tw < 0.75 ? "." : "·", tw < 0.15 ? 2 : h < 0.012 ? 1 : 0);
    }
    // A shooting star every ~7s, over in about a second and a half.
    const CYCLE = 70, k = Math.floor(t / CYCLE), p = t % CYCLE;
    if (p < 16) {
      const x0 = s.cols * (0.35 + 0.6 * hash(k, 21)), y0 = s.rows * 0.35 * hash(k, 22);
      const hx = x0 - p * 2.4, hy = y0 + p * 1.0;
      put(hx, hy, "*", 3);
      for (let j = 1; j <= 9; j++) put(hx + j * 2.4 * 0.5, hy - j * 0.5, j % 2 ? "-" : "~", j < 4 ? 2 : j < 7 ? 1 : 0);
    }
  },
};

/* ---------------- Decode (Fix Match) ---------------- */
// Lines of drifting characters; whatever is being matched (the file name,
// then the title you pick) unscrambles out of them, letter by letter.
function decode() {
  return {
    text: "", since: -1e9,
    setText(str) {
      str = String(str || "").replace(/\s+/g, " ").trim();
      if (!str || str === this.text) return;
      this.text = str; this.since = t;
      if (pattern === this) refresh();
    },
    frame(s, t) {
      const put = putter(s);
      // Sparse noise that slowly reshuffles.
      for (let r = 0; r < s.rows; r++) for (let c = 0; c < s.cols; c++) {
        if (hash(c, r, 3) < 0.05) put(c, r, NOISE[Math.floor(hash(c, r, t >> 2) * NOISE.length)], 0);
      }
      if (!this.text) return;
      const unit = [...(this.text + "   ·   ")];
      const age = reduce.matches ? 1e9 : t - this.since;
      for (let row = 2, k = 0; row < s.vrows - 1; row += 6, k++) {
        const dir = k % 2 ? 1 : -1, speed = 0.18 + 0.08 * (k % 3);
        const off = Math.floor(t * speed) * dir + k * 11;
        for (let c = 0; c < s.cols; c++) {
          const idx = wrapN(c + off, unit.length), ch = unit[idx];
          if (ch === " ") continue;
          // Each letter of each line locks in at its own moment in the first ~2s.
          if (age < 3 + hash(idx, k, 5) * 18) put(c, row, NOISE[Math.floor(hash(c, row, t) * NOISE.length)], 1);
          else put(c, row, ch, k % 2 ? 2 : 3);
        }
      }
    },
  };
}

return {
  /** Show this pattern (an object with frame(s, t) and optional init(s)). */
  use(p) {
    mount();
    if (p !== pattern) { pattern = p; p.init && p.init(s); }
    refresh();
  },
  setOn(v) { on = !!v; mount(); refresh(); },
  loadImage, artwork, decode, aquarium, sky,
};
})();
