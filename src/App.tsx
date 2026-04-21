import { useEffect, useState } from "react";
import { initWasm, parse, wasmPing } from "./lib/wasm-bridge";

const SAMPLE_CCM = `<![LOG[Client Health evaluation starts.]LOG]!><time="23:00:10.6893636" date="11-12-2025" component="ClientHealth" context="" type="1" thread="1" file="">
<![LOG[MdmDeviceCertificate retrieved successfully]LOG]!><time="23:00:11.4573058" date="11-12-2025" component="ClientHealth" context="" type="1" thread="1" file="">
<![LOG[Health evaluation failed: 0x80070005 access denied]LOG]!><time="23:00:12.0000000" date="11-12-2025" component="ClientHealth" context="" type="3" thread="1" file="">`;

type ParseResultLike = {
  entries: Array<{ message: string; component?: string | null; severity: unknown }>;
  formatDetected: unknown;
  totalLines: number;
  parseErrors: number;
};

type Sample = {
  version: string;
  entryCount: number;
  firstMessage: string;
  formatDetected: unknown;
  totalLines: number;
  parseErrors: number;
};

type State =
  | { tag: "loading" }
  | { tag: "ready"; sample: Sample }
  | { tag: "error"; message: string };

export default function App() {
  const [state, setState] = useState<State>({ tag: "loading" });

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        await initWasm();
        const version = await wasmPing();
        const result = (await parse(SAMPLE_CCM, "sample.log")) as ParseResultLike;
        if (cancelled) return;
        setState({
          tag: "ready",
          sample: {
            version,
            entryCount: result.entries.length,
            firstMessage: result.entries[0]?.message ?? "<no entries>",
            formatDetected: result.formatDetected,
            totalLines: result.totalLines,
            parseErrors: result.parseErrors,
          },
        });
      } catch (err) {
        if (cancelled) return;
        setState({ tag: "error", message: String(err) });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <main style={{ fontFamily: "system-ui, sans-serif", padding: 32, maxWidth: 720 }}>
      <h1 style={{ margin: 0 }}>CMTrace Open — Web</h1>
      <p style={{ color: "#666", marginTop: 4 }}>
        Browser log viewer. Proof-of-wire only — full viewer UI lands next.
      </p>

      <section style={{ marginTop: 32 }}>
        <h2 style={{ fontSize: 18, margin: "0 0 12px" }}>WASM parser</h2>
        {state.tag === "loading" && <p>Initializing WASM module…</p>}
        {state.tag === "error" && (
          <pre
            style={{
              color: "#b00",
              whiteSpace: "pre-wrap",
              background: "#fff0f0",
              padding: 12,
              borderRadius: 4,
            }}
          >
            {state.message}
          </pre>
        )}
        {state.tag === "ready" && (
          <dl style={{ display: "grid", gridTemplateColumns: "auto 1fr", gap: "4px 16px" }}>
            <dt>Version</dt>
            <dd>
              <code>{state.sample.version}</code>
            </dd>
            <dt>Format detected</dt>
            <dd>
              <code>{JSON.stringify(state.sample.formatDetected)}</code>
            </dd>
            <dt>Entries parsed</dt>
            <dd>
              {state.sample.entryCount} <span style={{ color: "#888" }}>(from {state.sample.totalLines} lines, {state.sample.parseErrors} parse errors)</span>
            </dd>
            <dt>First message</dt>
            <dd>
              <code style={{ whiteSpace: "pre-wrap" }}>{state.sample.firstMessage}</code>
            </dd>
          </dl>
        )}
      </section>
    </main>
  );
}
