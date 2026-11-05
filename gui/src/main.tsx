import { render } from "preact";
import { App } from "./App";
import { maybeShowHome } from "./components/Home";
import { connectWS, loadCatalog, refresh, refreshHistory, refreshJobs, refreshLogs, refreshProxies, toast } from "./store";
import "./styles.css";

// a bug anywhere in the editor shows up instead of failing silently (once per message)
let lastError = "";
function report(msg: string) {
  if (!msg || msg === lastError || /ResizeObserver loop/.test(msg)) return;
  lastError = msg;
  setTimeout(() => lastError === msg && (lastError = ""), 5000);
  console.error("MoodCut:", msg);
  toast(`Something went wrong in the editor: ${msg}. Your project is safe.`, true);
}
window.addEventListener("error", (e) => report(e.message));
window.addEventListener("unhandledrejection", (e) => report(e.reason instanceof Error ? e.reason.message : String(e.reason ?? "unknown error")));

async function boot() {
  try {
    await Promise.all([refresh(), loadCatalog(), refreshLogs(), refreshJobs(), refreshHistory(), refreshProxies()]);
  } catch (e) {
    toast(`Can't reach the MoodCut engine: ${e}`, true);
  }
  connectWS();
  maybeShowHome();
  render(<App />, document.getElementById("app")!);
  // the boot splash fades out once the editor is drawn
  const splash = document.getElementById("splash");
  if (splash) {
    splash.classList.add("gone");
    setTimeout(() => splash.remove(), 400);
  }
}

boot();
