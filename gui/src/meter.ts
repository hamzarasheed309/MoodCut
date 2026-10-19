// Peak meter for the program monitor's playback (Web Audio on the streamed <video>).
import { signal } from "@preact/signals";

/** Peak level per channel in dBFS (-100 = silence). */
export const levels = signal<[number, number]>([-100, -100]);

let ctx: AudioContext | null = null;
let split: ChannelSplitterNode | null = null;
const routed = new WeakSet<HTMLMediaElement>();

/** Routes the element through an analyser. Call from a user gesture (play); safe to call again. */
export function attachMeter(v: HTMLMediaElement) {
  try {
    if (!ctx) {
      const c = new AudioContext();
      const sp = c.createChannelSplitter(2);
      const an = [c.createAnalyser(), c.createAnalyser()];
      an.forEach((a, i) => {
        a.fftSize = 1024;
        sp.connect(a, i);
      });
      ctx = c;
      split = sp;
      const buf = new Float32Array(1024);
      let held: [number, number] = [-100, -100];
      const tick = () => {
        const now = an.map((a) => {
          a.getFloatTimeDomainData(buf);
          let p = 0;
          for (let i = 0; i < buf.length; i++) p = Math.max(p, Math.abs(buf[i]));
          return p > 1e-5 ? 20 * Math.log10(p) : -100;
        });
        // fast attack, ~20 dB/s release
        const next = now.map((d, i) => Math.max(d, held[i] - 0.35)) as [number, number];
        if (Math.abs(next[0] - held[0]) > 0.2 || Math.abs(next[1] - held[1]) > 0.2) levels.value = next;
        held = next;
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    }
    if (!routed.has(v)) {
      const src = ctx.createMediaElementSource(v);
      src.connect(ctx.destination);
      src.connect(split!);
      routed.add(v);
    }
    ctx.resume().catch(() => {});
  } catch {
    // no Web Audio: playback still works, just without a meter
  }
}
