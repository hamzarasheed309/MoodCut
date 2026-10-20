import { render } from "preact";
import { App } from "./App";
import { maybeShowHome } from "./components/Home";
import { connectWS, loadCatalog, refresh, refreshHistory, refreshJobs, refreshLogs, refreshProxies, toast } from "./store";
import "./styles.css";

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
