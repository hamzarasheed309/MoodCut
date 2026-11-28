// Audio Meters (Premiere Pro): the program's stereo peak level while it plays, on a dB scale
// from 0 down to -54, with a peak hold that falls back slowly and a red clip light above 0 dB.
import { useEffect, useRef, useState } from "preact/hooks";
import { levels } from "../meter";
import { playing } from "../store";

const FLOOR = -54;
const MARKS = [0, -6, -12, -18, -24, -30, -36, -42, -48, -54];
const pos = (db: number) => Math.max(0, Math.min(1, (db - FLOOR) / -FLOOR));

export function MetersPanel() {
  const [hold, setHold] = useState<[number, number]>([-100, -100]);
  const [clip, setClip] = useState(false);
  const last = useRef(0);
  const l = levels.value;
  useEffect(() => {
    const now = performance.now();
    setHold((h) => {
      const fall = (now - last.current) / 1000 > 1.5;
      return [Math.max(l[0], fall ? h[0] - 3 : h[0]), Math.max(l[1], fall ? h[1] - 3 : h[1])] as [number, number];
    });
    if (l[0] > -0.1 || l[1] > -0.1) setClip(true);
    if (Math.max(l[0], l[1]) > Math.max(hold[0], hold[1]) - 0.5) last.current = now;
  }, [l[0], l[1]]);
  const quiet = !playing.value && l[0] < -90 && l[1] < -90;
  return (
    <div class="meters" title={`Peak L ${l[0] > -90 ? l[0].toFixed(1) : "-∞"} dB · R ${l[1] > -90 ? l[1].toFixed(1) : "-∞"} dB`}>
      <div class={"meters-clip" + (clip ? " on" : "")} title="Clipped above 0 dB: click to reset" onClick={() => setClip(false)} />
      <div class="meters-body">
        <div class="meters-scale">
          {MARKS.map((m) => (
            <span key={m} style={{ top: `${(1 - pos(m)) * 100}%` }}>
              {m === 0 ? "0" : m}
            </span>
          ))}
        </div>
        {[0, 1].map((i) => (
          <div class={"meters-ch" + (quiet ? " idle" : "")} key={i}>
            <div class="meters-fill" style={{ clipPath: `inset(${(1 - pos(l[i])) * 100}% 0 0 0)` }} />
            {hold[i] > FLOOR && <div class="meters-hold" style={{ bottom: `${pos(hold[i]) * 100}%` }} />}
          </div>
        ))}
      </div>
      <div class="meters-foot">
        <span>L</span>
        <span>R</span>
      </div>
      <div class="meters-db">dB</div>
    </div>
  );
}
