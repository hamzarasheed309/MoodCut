import { useEffect, useState } from "preact/hooks";
import { ffVersion, refresh, toast } from "../store";

type Setup = { installed: boolean; version: string; running: boolean; status: string; error: string | null };

async function setup(action: "start" | "status"): Promise<Setup> {
  const r = await fetch("/api/setup/ffmpeg", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ action }) });
  return r.json();
}

/** Shown while FFmpeg is missing: one click downloads a portable build. */
export function FfmpegBanner() {
  const [st, setSt] = useState<Setup | null>(null);
  const [hidden, setHidden] = useState(false);
  const missing = ffVersion.value === "not installed";

  useEffect(() => {
    if (!st?.running) return;
    const h = setInterval(async () => {
      const s = await setup("status");
      setSt(s);
      if (s.installed) {
        toast(s.status || `FFmpeg ${s.version} ready`);
        refresh();
      } else if (s.error) toast(s.error, true);
    }, 700);
    return () => clearInterval(h);
  }, [st?.running]);

  if (!missing || hidden) return null;
  const busy = !!st?.running;
  return (
    <div class="ff-banner">
      <div>
        <b>FFmpeg isn't installed.</b> Importing, playing and exporting video need it. MoodCut can download a portable copy (about 100 MB, no admin rights).
        {busy && <div class="ff-status">{st?.status || "starting"}…</div>}
        {st?.error && !busy && <div class="ff-status err">{st.error}</div>}
      </div>
      <button class="btn sm primary" disabled={busy} onClick={async () => setSt(await setup("start"))}>
        {busy ? "Installing…" : st?.error ? "Try again" : "Install FFmpeg"}
      </button>
      <button class="btn sm ghost" title="Hide (graphics-only work still runs)" onClick={() => setHidden(true)}>
        ✕
      </button>
    </div>
  );
}
