// MoodCut live preview bridge. Runs right after gfx_shim.js, only in the editor's
// preview of an HTML graphic (a sandboxed iframe), and lets the editor drive the
// virtual clock with postMessage:
//   editor -> page  {mcPreview: 1, type: 'seek', ms, mode: 'exact' | 'quick', seq}
//   page -> editor  ready {ms} | seeked {ms, seq} | rewind {ms, seq} | error {msg} | pick {texts}
// An exact seek plays frame by frame up to the time like the renderer does, so
// animations driven by requestAnimationFrame or timers match the final render. Going
// back in time needs a fresh page: the bridge answers 'rewind' and the editor reloads
// the page at that time. A quick seek jumps straight there (scrubbing); the next
// exact seek reloads if the page keeps state of its own.
(() => {
  const mc = window.__mc;
  if (!mc || window.__mcPreview) return;
  window.__mcPreview = true;
  const MC = window.MC || {};
  const post = (m) => {
    try {
      parent.postMessage(Object.assign({ mcPreview: 1 }, m), '*');
    } catch (e) {}
  };
  const say = (msg) => post({ type: 'error', msg: String(msg).split('\n')[0].slice(0, 300) });
  addEventListener(
    'error',
    (e) => {
      const el = e.target;
      if (el && el !== window && (el.src || el.href)) say('could not load ' + (el.src || el.href));
      else say((e.message || 'error') + (e.lineno ? ' (line ' + e.lineno + ')' : ''));
    },
    true
  );
  addEventListener('unhandledrejection', (e) => say('unhandled promise rejection: ' + ((e.reason && e.reason.message) || e.reason)));
  const realError = console.error.bind(console);
  console.error = (...a) => {
    say(a.map((x) => (x && x.stack ? x.stack : String(x))).join(' '));
    realError(...a);
  };

  const frame = 1000 / (MC.fps || 30);
  let cur = 0; // virtual ms the page has played to
  let exact = true; // false after a quick jump in a page that keeps state of its own
  const clockFree = () => typeof mc.ticks === 'function' && mc.ticks() === 0;

  async function playTo(ms) {
    if (clockFree()) {
      await mc.seek(ms);
    } else {
      for (let t = cur + frame, n = 0; t < ms - 0.01 && n < 36000; t += frame, n++) await mc.seek(t);
      await mc.seek(ms);
    }
    cur = ms;
  }

  async function seek(ms, mode) {
    ms = Math.max(0, Number(ms) || 0);
    if (mode === 'quick' || clockFree()) {
      await mc.seek(ms);
      if (!clockFree()) exact = false;
      cur = ms;
      return 'seeked';
    }
    if (ms < cur - 0.01 || !exact) return 'rewind';
    await playTo(ms);
    return 'seeked';
  }

  let queue = (async () => {
    try {
      await mc.ready(15000);
    } catch (e) {
      say(e);
    }
    const m = /[#&]t=([\d.]+)/.exec(location.hash);
    const t = m ? parseFloat(m[1]) : 0;
    try {
      await playTo(t);
    } catch (e) {
      say(e);
    }
    post({ type: 'ready', ms: t, clock: !clockFree() });
  })();

  addEventListener('message', (e) => {
    const d = e.data;
    if (e.source !== parent || !d || !d.mcPreview || d.type !== 'seek') return;
    queue = queue
      .then(() => seek(d.ms, d.mode))
      .then(
        (r) => post({ type: r, ms: d.ms, seq: d.seq }),
        (err) => {
          say(err);
          post({ type: 'seeked', ms: d.ms, seq: d.seq });
        }
      );
  });

  // clicking a text selects its field in the editor: send the texts from the
  // clicked element up to the page, the editor matches them with field values
  addEventListener(
    'click',
    (e) => {
      e.preventDefault();
      e.stopPropagation();
      const texts = [];
      for (let el = e.target; el && el !== document.documentElement && texts.length < 8; el = el.parentElement) {
        const t = (el.textContent || '').replace(/\s+/g, ' ').trim();
        if (t && !texts.includes(t)) texts.push(t.slice(0, 500));
      }
      post({ type: 'pick', texts });
    },
    true
  );
})();
