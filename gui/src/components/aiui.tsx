// Shared pieces of the AI panels (AI Voice, AI Video) and the AI Services window: the task
// model, progress bars that tell a measured percentage from an estimate, problems explained
// with the button that fixes them, and API key entry. Keys go to the engine (stored encrypted
// for this Windows user) and never come back to the page: it sees only "connected" and the
// key's last characters.
import type { ComponentChildren } from "preact";
import { useEffect, useState } from "preact/hooks";
import { api } from "../gfx";
import { servicesTick, showServices, toast } from "../store";

export type AiTask = {
  id: number;
  lane: "voice" | "video";
  kind: string;
  provider: string;
  label: string;
  state: "queued" | "running" | "done" | "error";
  status: string;
  pct: number | null;
  estimated: boolean;
  eta: number | null;
  cost: string | null;
  error: string | null;
  hint: string | null;
  fix: "retry" | "keys" | "billing" | null;
  link: string | null;
  detail: string | null;
  result: any;
  secs: number;
};

export type Service = {
  id: string;
  name: string;
  what: string;
  keys_url: string;
  billing_url: string;
  env: string;
  connected: boolean;
  source: string | null;
  masked: string | null;
};

export const clock = (s: number) => {
  const n = Math.max(0, Math.round(s));
  return n >= 60 ? `${Math.floor(n / 60)}:${String(n % 60).padStart(2, "0")}` : `${n} s`;
};

/** A progress bar: a fraction, or a moving stripe while the amount is unknown. */
export function Bar({ pct, estimated }: { pct: number | null; estimated?: boolean }) {
  return (
    <div class={"ai-bar" + (pct == null ? " indet" : "") + (estimated ? " est" : "")}>
      <div style={{ width: pct == null ? "30%" : `${Math.max(2, Math.round(pct * 100))}%` }} />
    </div>
  );
}

export function ExtLink({ href, children }: { href: string; children: ComponentChildren }) {
  return (
    <a class="ai-link" href={href} target="_blank" rel="noopener noreferrer">
      {children}
    </a>
  );
}

/** An error people can act on: what happened, what to do, the button that helps, the details. */
export function Problem({ message, hint, detail, fix, link, provider, onRetry }: { message: string; hint?: string | null; detail?: string | null; fix?: AiTask["fix"]; link?: string | null; provider?: string; onRetry?: () => void }) {
  return (
    <div class="ai-problem" role="alert">
      <div class="ai-problem-msg">{message}</div>
      {hint && <div class="ai-problem-hint">{hint}</div>}
      <div class="btn-row">
        {fix === "keys" && (
          <button class="btn sm primary" onClick={() => (showServices.value = provider || "")}>
            Open AI Services
          </button>
        )}
        {fix === "billing" && link && (
          <a class="btn sm primary" href={link} target="_blank" rel="noopener noreferrer">
            Open billing ↗
          </a>
        )}
        {onRetry && (
          <button class={"btn sm" + (fix === "retry" ? " primary" : "")} onClick={onRetry}>
            Retry
          </button>
        )}
        {link && fix !== "billing" && <ExtLink href={link}>Get a key ↗</ExtLink>}
      </div>
      {detail && detail !== message && (
        <details class="ai-details">
          <summary>Details</summary>
          <pre>{detail}</pre>
        </details>
      )}
    </div>
  );
}

/** One job: its stage and progress while it runs (measured, or ~estimated from the model's usual
 * time), then its result or its explained error. */
export function TaskCard({ t, onCancel, onRetry, onDismiss, children }: { t: AiTask; onCancel: () => void; onRetry: () => void; onDismiss?: () => void; children?: ComponentChildren }) {
  const live = t.state === "running" || t.state === "queued";
  return (
    <div class={"ai-task " + t.state}>
      <div class="ai-task-top">
        <span class="ai-sdot" />
        <span class="ai-task-label" title={t.label}>
          {t.label}
        </span>
        {t.cost && <span class="ai-cost" title="What this job costs on your account">{t.cost}</span>}
        {live && (
          <button class="btn sm ghost icon" title="Cancel (also stops it at the provider)" onClick={onCancel}>
            ✕
          </button>
        )}
        {!live && onDismiss && (
          <button class="btn sm ghost icon" title="Hide (it stays in the list below)" onClick={onDismiss}>
            ✕
          </button>
        )}
      </div>
      {t.state === "running" && (
        <div class="ai-task-body">
          <Bar pct={t.pct} estimated={t.estimated} />
          <div class="ai-task-status">
            <span>{t.status}</span>
            <span class="ai-faint" title={t.estimated ? "Estimated from how long this model usually takes: the provider reports no percentage" : "Reported by the provider"}>
              {t.pct != null ? `${t.estimated ? "~" : ""}${Math.round(t.pct * 100)}%` : ""}
              {t.eta != null && t.eta > 0 ? ` · ${clock(t.eta)} left` : ""}
              {` · ${clock(t.secs)}`}
            </span>
          </div>
        </div>
      )}
      {t.state === "queued" && <div class="ai-task-status ai-faint">{t.status}</div>}
      {t.state === "done" && (
        <div class="ai-task-status">
          <span>{t.status}</span>
          <span class="ai-faint">{clock(t.secs)}</span>
        </div>
      )}
      {t.state === "error" &&
        (t.error === "Cancelled." ? (
          <div class="ai-task-status ai-faint">Cancelled.</div>
        ) : (
          <Problem message={t.error || "Failed."} hint={t.hint} detail={t.detail} fix={t.fix} link={t.link} provider={t.provider} onRetry={onRetry} />
        ))}
      {children}
    </div>
  );
}

/** The running job (or the latest result until dismissed), then the history. */
export function TaskList({ tasks, render, onCancel, onRetry, onClear }: { tasks: AiTask[]; render?: (t: AiTask) => ComponentChildren; onCancel: (id?: number) => void; onRetry: (id: number) => void; onClear: () => void }) {
  const [dismissed, setDismissed] = useState<number | null>(null);
  if (!tasks.length) return null;
  const running = tasks.filter((t) => t.state === "running");
  const queued = tasks.filter((t) => t.state === "queued");
  const last = tasks[tasks.length - 1];
  const pinned = running.length ? running : last && last.state !== "queued" && last.id !== dismissed ? [last] : [];
  const rest = tasks.filter((t) => !pinned.includes(t) && t.state !== "queued");
  const card = (t: AiTask, dismiss?: boolean) => (
    <TaskCard key={t.id} t={t} onCancel={() => onCancel(t.id)} onRetry={() => onRetry(t.id)} onDismiss={dismiss ? () => setDismissed(t.id) : undefined}>
      {render?.(t)}
    </TaskCard>
  );
  return (
    <>
      {pinned.length > 0 && <div class="ai-pinned">{pinned.map((t) => card(t, !running.length))}</div>}
      {queued.length > 0 && (
        <div class="ai-faint ai-queued">
          {queued.length} waiting ·{" "}
          <button class="btn sm ghost" onClick={() => queued.forEach((q) => onCancel(q.id))}>
            cancel them
          </button>
        </div>
      )}
      {rest.length > 0 && (
        <details class="ai-history" open={rest.length <= 3}>
          <summary>
            History ({rest.length})
            <button
              class="btn sm ghost"
              onClick={(e) => {
                e.preventDefault();
                onClear();
              }}
            >
              Clear
            </button>
          </summary>
          {[...rest].reverse().map((t) => card(t))}
        </details>
      )}
    </>
  );
}

// ---------------------------------------------------------------- API keys

type ServicesReply = { providers: Service[]; test: { ok: boolean; text: string; hint?: string } | null };

export async function servicesApi(body?: Record<string, unknown>) {
  return api<ServicesReply>("/api/ai/services", body ?? { action: "status" });
}

/** Paste a key, save it (it is tested at once), or remove it. */
export function KeyEntry({ s, onChange, compact }: { s: Service; onChange: (list: Service[]) => void; compact?: boolean }) {
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<{ ok: boolean; text: string; hint?: string } | null>(null);
  const go = async (body: Record<string, unknown>) => {
    setBusy(true);
    const r = await servicesApi({ ...body, provider: s.id });
    setBusy(false);
    if (!r.ok) return toast(r.error, true);
    onChange(r.providers);
    setNote(r.test);
    servicesTick.value++;
    if (body.action === "set_key") setKey("");
  };
  return (
    <div class={"ai-key" + (compact ? " compact" : "")}>
      {s.connected ? (
        <div class="ai-key-row">
          <span class="ai-chip ok">Connected</span>
          <code title={s.source || ""}>{s.masked}</code>
          <span class="ai-faint">{s.source === "saved in MoodCut" ? "saved, encrypted" : `from ${s.source}`}</span>
          <span style={{ flex: 1 }} />
          <button class="btn sm" disabled={busy} onClick={() => go({ action: "test_key" })}>
            {busy ? "Testing…" : "Test"}
          </button>
          {s.source === "saved in MoodCut" && (
            <button class="btn sm ghost" disabled={busy} onClick={() => confirm(`Remove the ${s.name} key from MoodCut?`) && go({ action: "remove_key" })}>
              Remove
            </button>
          )}
        </div>
      ) : null}
      <div class="ai-key-row">
        <input
          type="password"
          autocomplete="off"
          spellcheck={false}
          placeholder={s.connected ? `Replace the ${s.name} key…` : `Paste your ${s.name} API key`}
          value={key}
          onInput={(e) => setKey((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter" && key.trim()) go({ action: "set_key", key });
          }}
        />
        <button class="btn sm primary" disabled={busy || !key.trim()} onClick={() => go({ action: "set_key", key })}>
          {busy ? "Checking…" : "Save & test"}
        </button>
      </div>
      {note && <div class={"ai-note " + (note.ok ? "ok" : "err")}>{note.text}{note.hint ? ` ${note.hint}` : ""}</div>}
      {!compact && (
        <div class="ai-faint ai-key-links">
          <ExtLink href={s.keys_url}>Get a key ↗</ExtLink> · <ExtLink href={s.billing_url}>Plans and credits ↗</ExtLink> · or set <code>{s.env}</code> before starting MoodCut
        </div>
      )}
    </div>
  );
}

/** Shown in a panel while its service has no key. */
export function ConnectCard({ id, title, lines }: { id: string; title: string; lines: string[] }) {
  const [list, setList] = useState<Service[] | null>(null);
  useEffect(() => {
    servicesApi().then((r) => r.ok && setList(r.providers));
  }, []);
  const s = list?.find((x) => x.id === id);
  return (
    <div class="ai-connect">
      <div class="ai-connect-title">{title}</div>
      <ul>
        {lines.map((l) => (
          <li key={l}>{l}</li>
        ))}
      </ul>
      {s && <KeyEntry s={s} onChange={setList} />}
    </div>
  );
}

/** Edit > AI Services: every service, its key and what MoodCut uses it for. */
export function AIServices() {
  const focus = showServices.value;
  const [list, setList] = useState<Service[] | null>(null);
  useEffect(() => {
    if (focus === null) return;
    servicesApi().then((r) => (r.ok ? setList(r.providers) : toast(r.error, true)));
  }, [focus]);
  if (focus === null) return null;
  const close = () => (showServices.value = null);
  return (
    <div class="modal-bg" onPointerDown={(e) => e.target === e.currentTarget && close()} onKeyDown={(e) => e.key === "Escape" && close()}>
      <div class="modal ai-services" role="dialog" aria-label="AI Services">
        <div class="modal-h">
          AI Services
          <span class="x" onClick={close} title="Close">
            ✕
          </span>
        </div>
        <div class="modal-b">
          <p class="ai-faint" style={{ marginTop: 0 }}>
            MoodCut's AI Voice and AI Video run on these services with your own accounts. Keys are stored encrypted for your Windows user and are never shown to AI apps or web pages; usage is billed by each
            service.
          </p>
          {!list && <div class="ai-faint">Loading…</div>}
          {list?.map((s) => (
            <div key={s.id} class={"ai-service" + (focus === s.id ? " focus" : "")}>
              <div class="ai-service-h">
                <b>{s.name}</b>
                <span class="ai-faint">{s.what}</span>
              </div>
              <KeyEntry s={s} onChange={setList} />
            </div>
          ))}
        </div>
        <div class="modal-f">
          <button class="btn sm" onClick={close}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
