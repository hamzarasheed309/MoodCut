import { useEffect, useRef, useState } from "preact/hooks";
import { logs, run } from "../store";

/** Activity log (AI + GUI edits) and a MoodCut Script prompt. */
export function Console() {
  const [cmd, setCmd] = useState("");
  const [hist, setHist] = useState<string[]>([]);
  const [hi, setHi] = useState(-1);
  const ref = useRef<HTMLDivElement>(null);
  const entries = logs.value.slice().reverse();
  useEffect(() => {
    ref.current?.scrollTo({ top: ref.current.scrollHeight });
  }, [entries.length]);
  const submit = async () => {
    const s = cmd.trim();
    if (!s) return;
    setHist([s, ...hist.filter((h) => h !== s)].slice(0, 50));
    setHi(-1);
    const r = await run(s, true);
    if (r.ok) setCmd("");
  };
  return (
    <div class="console">
      <div class="log" ref={ref}>
        {entries.length === 0 && (
          <div class="empty">
            Edits from your AI assistant (MCP) and from this window appear here.
            <br />
            Type MoodCut Script below, e.g. <span class="kbd">text 'Hello' at=0 dur=3 enter=pop</span>
          </div>
        )}
        {entries.map((e, i) => (
          <div class={"log-e" + (e.ok ? "" : " err")} key={i}>
            <div>
              <span class={"src " + (e.source === "mcp" ? "mcp" : e.source === "gui" ? "gui" : "cli")}>{e.source === "mcp" ? "AI" : e.source}</span>
              <span style={{ color: "var(--faint)" }}>rev {e.rev}</span>
            </div>
            <pre style={{ color: "var(--text)" }}>{e.script.length > 600 ? e.script.slice(0, 600) + " …" : e.script}</pre>
            <pre style={{ color: e.ok ? "var(--muted)" : "var(--danger)" }}>{e.result.length > 800 ? e.result.slice(0, 800) + " …" : e.result}</pre>
          </div>
        ))}
      </div>
      <div class="cmd">
        <textarea
          value={cmd}
          placeholder="MoodCut Script — Enter to run, Shift+Enter for a new line, ↑ history"
          onInput={(e) => setCmd((e.target as HTMLTextAreaElement).value)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            } else if (e.key === "ArrowUp" && !cmd.includes("\n") && hist.length) {
              e.preventDefault();
              const n = Math.min(hist.length - 1, hi + 1);
              setHi(n);
              setCmd(hist[n]);
            } else if (e.key === "ArrowDown" && hi >= 0) {
              e.preventDefault();
              const n = hi - 1;
              setHi(n);
              setCmd(n >= 0 ? hist[n] : "");
            }
          }}
        />
        <button class="btn" onClick={submit}>
          Run
        </button>
      </div>
    </div>
  );
}
