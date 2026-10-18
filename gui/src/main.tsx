import { render } from "preact";
import { App } from "./App";
import { connectWS, loadCatalog, refresh, refreshJobs, refreshLogs, toast } from "./store";
import "./styles.css";

async function boot() {
  try {
    await Promise.all([refresh(), loadCatalog(), refreshLogs(), refreshJobs()]);
  } catch (e) {
    toast(`Can't reach the MoodCut engine: ${e}`, true);
  }
  connectWS();
  render(<App />, document.getElementById("app")!);
}

boot();
