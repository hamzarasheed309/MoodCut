// MoodCut virtual clock for HTML motion graphics. Injected before any page
// script so that every animation system reads the same frame time:
// performance.now / Date / requestAnimationFrame / setTimeout / setInterval /
// document.timeline are virtual, and CSS animations, CSS transitions, Web
// Animations, SVG (SMIL) and <video> are paused and seeked to each frame.
// MoodCut calls __mc.ready() once, then __mc.seek(ms) before every screenshot.
(() => {
  if (window.__mc) return;
  const MC = window.MC || {};
  window.MC = MC;
  let now = 0; // virtual milliseconds since the graphic started
  const epoch = Date.now();
  const RealDate = Date;
  const realSetTimeout = window.setTimeout.bind(window);
  const realNow = performance.now.bind(performance);

  // ---- clocks
  function MCDate(...a) {
    if (!new.target) return new RealDate(epoch + now).toString();
    return a.length ? new RealDate(...a) : new RealDate(epoch + now);
  }
  MCDate.prototype = RealDate.prototype;
  MCDate.now = () => epoch + now;
  MCDate.parse = RealDate.parse;
  MCDate.UTC = RealDate.UTC;
  window.Date = MCDate;
  performance.now = () => now;
  try {
    Object.defineProperty(document.timeline, 'currentTime', { get: () => now, configurable: true });
  } catch (e) {}
  Object.defineProperty(MC, 'time', { get: () => now / 1000, configurable: true });

  // ---- frames and timers
  const rafs = new Map();
  let rafSeq = 0;
  window.requestAnimationFrame = (cb) => {
    rafs.set(++rafSeq, cb);
    return rafSeq;
  };
  window.cancelAnimationFrame = (id) => {
    rafs.delete(id);
  };
  const timers = new Map();
  let timerSeq = 0;
  const addTimer = (cb, ms, args, repeat) => {
    const d = Math.max(repeat ? 1 : 0, Number(ms) || 0);
    timers.set(++timerSeq, { id: timerSeq, at: now + d, cb, args, every: repeat ? d : 0 });
    return timerSeq;
  };
  window.setTimeout = (cb, ms, ...args) => addTimer(cb, ms, args, false);
  window.setInterval = (cb, ms, ...args) => addTimer(cb, ms, args, true);
  window.clearTimeout = window.clearInterval = (id) => {
    timers.delete(id);
  };
  const run = (fn, args) => {
    try {
      if (typeof fn === 'function') fn(...(args || []));
      else (0, eval)(String(fn));
    } catch (e) {
      console.error(e && e.stack ? e.stack : String(e));
    }
  };

  // ---- animations, SVG, video
  const born = new WeakMap();
  function syncAnimations() {
    let list = [];
    try {
      list = document.getAnimations();
    } catch (e) {}
    for (const a of list) {
      if (!born.has(a)) born.set(a, now);
      try {
        if (a.playState !== 'paused') a.pause();
        a.currentTime = now - born.get(a);
      } catch (e) {}
    }
    for (const s of document.querySelectorAll('svg')) {
      try {
        if (s.pauseAnimations) s.pauseAnimations();
        if (s.setCurrentTime) s.setCurrentTime(now / 1000);
      } catch (e) {}
    }
  }
  function syncVideos() {
    const vids = [...document.querySelectorAll('video')];
    return Promise.all(
      vids.map(
        (v) =>
          new Promise((res) => {
            if (!born.has(v)) born.set(v, now);
            try {
              v.pause();
            } catch (e) {}
            let t = ((now - born.get(v)) / 1000) * (v.playbackRate || 1);
            const d = v.duration;
            if (isFinite(d) && d > 0) t = v.loop ? t % d : Math.min(t, d - 0.001);
            if (Math.abs(v.currentTime - t) < 0.0005 && v.readyState >= 2) return res();
            let done = false;
            const fin = () => {
              if (done) return;
              done = true;
              v.removeEventListener('seeked', fin);
              res();
            };
            v.addEventListener('seeked', fin);
            realSetTimeout(fin, 3000);
            v.currentTime = Math.max(0, t);
          })
      )
    );
  }
  const within = (p, ms) => Promise.race([p, new Promise((r) => realSetTimeout(r, ms))]);

  window.__mc = {
    async ready(timeoutMs) {
      const t0 = realNow();
      if (document.readyState !== 'complete') await within(new Promise((r) => addEventListener('load', r, { once: true })), timeoutMs);
      if (document.fonts) await within(document.fonts.ready, timeoutMs);
      const imgs = [...document.images].filter((i) => !i.complete);
      const loaded = (i) =>
        new Promise((r) => {
          i.addEventListener('load', r, { once: true });
          i.addEventListener('error', r, { once: true });
        });
      await within(Promise.all(imgs.map(loaded)), timeoutMs);
      for (const v of document.querySelectorAll('video')) {
        if (v.readyState < 2) await within(new Promise((r) => v.addEventListener('loadeddata', r, { once: true })), timeoutMs);
      }
      if (MC.ready && typeof MC.ready.then === 'function') await within(MC.ready, timeoutMs);
      return Math.round(realNow() - t0);
    },
    async seek(ms) {
      // run due timers in time order; each sees the clock at its own due time
      for (let guard = 0; guard < 200000; guard++) {
        let next = null;
        for (const t of timers.values()) if (t.at <= ms && (!next || t.at < next.at || (t.at === next.at && t.id < next.id))) next = t;
        if (!next) break;
        now = Math.max(now, next.at);
        if (next.every) next.at += next.every;
        else timers.delete(next.id);
        run(next.cb, next.args);
        syncAnimations();
        await Promise.resolve();
      }
      now = ms;
      const cbs = [...rafs.values()];
      rafs.clear();
      for (const cb of cbs) run(cb, [now]);
      // GSAP skips ticks shorter than its frame gap (so frame 0 would miss
      // .from() start states); a manual tick renders it at exactly `now`.
      if (window.gsap && window.gsap.ticker && typeof window.gsap.ticker.tick === 'function') run(() => window.gsap.ticker.tick());
      if (typeof MC.onFrame === 'function') run(MC.onFrame, [now / 1000]);
      syncAnimations();
      await syncVideos();
      await new Promise((r) => realSetTimeout(r, 0));
      syncAnimations();
      return now;
    },
    // bounding box of everything visible (for the "fit" check)
    bounds() {
      let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
      for (const el of document.body.querySelectorAll('*')) {
        const r = el.getBoundingClientRect();
        if (r.width < 1 || r.height < 1) continue;
        const cs = getComputedStyle(el);
        if (cs.visibility === 'hidden' || cs.display === 'none') continue;
        x0 = Math.min(x0, r.left); y0 = Math.min(y0, r.top); x1 = Math.max(x1, r.right); y1 = Math.max(y1, r.bottom);
      }
      return isFinite(x0) ? [x0, y0, x1, y1] : null;
    },
  };
})();
