import { useEffect, useState } from "preact/hooks";
import { refresh, refreshLogs, showCheck, toast } from "../store";

/** Runs the edit quality check and offers its fixes. */
export function CheckPanel() {
  const [busy, setBusy] = useState(false);
  const [target, setTarget] = useState("web");
  const [res, setRes] = useState<{ text: string; sheet?: string | null } | null>(null);
  const run = async (fix: boolean) => {
    setBusy(true);
    try {
      const r = await fetch("/api/check", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ target, fix }) }).then((x) => x.json());
      if (!r.ok) toast(r.text, true);
      setRes(r);
      if (fix) {
        await refresh();
        refreshLogs();
        // show the state after the fixes
        const again = await fetch("/api/check", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ target }) }).then((x) => x.json());
        setRes({ text: r.text.split("\n").filter((l: string) => l.startsWith("applied") || l.startsWith("!")).join("\n") + "\n\n" + again.text, sheet: again.sheet });
      }
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    if (showCheck.value) run(false);
  }, [showCheck.value]);
  if (!showCheck.value) return null;
  const close = () => ((showCheck.value = false), setRes(null));
  const fixes = res?.text.includes("fix line(s) available");
  return (
    <div class="modal-bg" onClick={close}>
      <div class="modal check-panel" onClick={(e) => e.stopPropagation()}>
        <div class="modal-h">
          Check edit
          <select value={target} onChange={(e) => setTarget((e.target as HTMLSelectElement).value)} style={{ marginLeft: "12px" }}>
            <option value="web">Web (−14 LUFS)</option>
            <option value="youtube">YouTube (−14 LUFS, pacing)</option>
            <option value="reels">Instagram Reels (UI zones, hook)</option>
            <option value="tiktok">TikTok (UI zones, hook)</option>
            <option value="shorts">YouTube Shorts (UI zones, hook)</option>
            <option value="podcast">Podcast (−16 LUFS)</option>
            <option value="broadcast">Broadcast EBU R128 (−23 LUFS)</option>
          </select>
          <button class="btn ghost icon x" onClick={close}>
            ✕
          </button>
        </div>
        <div class="modal-b">
          {busy && !res && <div class="empty">Checking…</div>}
          {res?.sheet && <img class="gfx-sheet" src={res.sheet} alt="frames with problems" />}
          {res && <pre class="gfx-log check-log">{res.text}</pre>}
        </div>
        <div class="modal-f">
          <button class="btn" onClick={close}>
            Close
          </button>
          <button class="btn" disabled={busy} onClick={() => run(false)}>
            {busy ? "Checking…" : "Check again"}
          </button>
          <button class="btn primary" disabled={busy || !fixes} onClick={() => run(true)}>
            Apply fixes
          </button>
        </div>
      </div>
    </div>
  );
}
