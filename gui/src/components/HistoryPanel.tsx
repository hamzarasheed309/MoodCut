// History panel: the undo stack (oldest at the top) and the redo stack, greyed. Click a step to go back to it.
import { history, run } from "../store";

export function HistoryPanel() {
  const { undo, redo } = history.value;
  const past = undo.slice().reverse();
  return (
    <div class="scroll history">
      <div class={"hist-row" + (undo.length === 0 ? " current" : "")} onClick={() => undo.length && run(`undo ${undo.length}`, true)}>
        <span class="dot" />
        Start
      </div>
      {past.map((label, i) => {
        const back = past.length - 1 - i;
        return (
          <div key={i} class={"hist-row" + (back === 0 ? " current" : "")} title={label} onClick={() => back > 0 && run(`undo ${back}`, true)}>
            <span class="dot" />
            {label}
          </div>
        );
      })}
      {redo.map((label, i) => (
        <div key={"r" + i} class="hist-row future" title={`${label} (click to redo up to here)`} onClick={() => run(`redo ${i + 1}`, true)}>
          <span class="dot" />
          {label}
        </div>
      ))}
      <div class="hint" style={{ padding: "10px" }}>
        Edits from the AI (over MCP) and from this window share one history. <span class="kbd">Ctrl+Z</span> / <span class="kbd">Ctrl+Shift+Z</span>
      </div>
    </div>
  );
}
