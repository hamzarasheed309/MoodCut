// Keeps a bug in one panel from blanking the whole editor: that panel shows what went wrong and
// a button to draw it again, and everything else keeps working. The project itself lives in the
// engine (and autosaves there), so nothing is lost when a panel fails.
import { Component } from "preact";
import type { ComponentChildren } from "preact";

type Props = { name: string; floating?: boolean; children?: ComponentChildren };
type State = { error: Error | null };

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  componentDidCatch(error: unknown) {
    console.error(`MoodCut: the ${this.props.name} panel failed`, error);
  }

  render() {
    const e = this.state.error;
    if (!e) return <>{this.props.children}</>;
    return (
      <div class={"panel-error" + (this.props.floating ? " floating" : "")} role="alert">
        <b>{this.props.name} hit a problem</b>
        <span>{e.message || String(e)}</span>
        <small>Your project is safe: it's kept and autosaved by the MoodCut engine, not by this panel.</small>
        <button class="btn sm" onClick={() => this.setState({ error: null })}>
          Reload {this.props.floating ? "" : "panel"}
        </button>
      </div>
    );
  }
}
