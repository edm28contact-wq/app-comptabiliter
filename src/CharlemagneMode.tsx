import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

type ConnectorStatus = {
  mode: string;
  version_label: string;
  transport_label: string;
  live_ready: boolean;
  preparation_ready: boolean;
  blocked_reason: string | null;
  switch_available: boolean;
};

type SyncPreview = {
  path: string;
  file_name: string;
  kind: string;
  line_count: number;
  column_count: number;
  separator: string | null;
  headers: string[];
  rows: string[][];
  raw_preview: string;
  duplicate: boolean;
};

const panelStyle: React.CSSProperties = {
  position: "fixed",
  right: 18,
  bottom: 18,
  zIndex: 80,
  width: 360,
  padding: 14,
  border: "1px solid #dfe4eb",
  borderRadius: 14,
  background: "#ffffff",
  boxShadow: "0 14px 40px rgba(20,31,50,.16)",
  color: "#172033",
  fontFamily: "Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
};

const buttonStyle: React.CSSProperties = {
  flex: 1,
  border: "1px solid #d7dde6",
  borderRadius: 9,
  padding: "9px 10px",
  background: "white",
  color: "#445064",
  fontWeight: 800,
  cursor: "pointer",
};

export default function CharlemagneMode() {
  const [status, setStatus] = useState<ConnectorStatus | null>(null);
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void invoke<ConnectorStatus>("get_charlemagne_connector_status")
      .then(setStatus)
      .catch((reason) => setError(String(reason)));
  }, []);

  const switchMode = async (mode: "import_file_v1" | "sync_files_v2" | "api_v3") => {
    if (busy || status?.mode === mode) return;
    setBusy(true);
    setError(null);
    setPreview(null);
    try {
      const next = await invoke<ConnectorStatus>("set_charlemagne_connector_mode", { mode });
      setStatus(next);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const importSyncFile = async () => {
    if (busy) return;
    const selected = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "Export Charlemagne", extensions: ["pdf", "csv", "tsv", "txt"] }],
    });
    if (!selected || Array.isArray(selected)) return;
    setBusy(true);
    setError(null);
    try {
      const result = await invoke<SyncPreview>("import_charlemagne_sync_file", { path: selected });
      setPreview(result);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  if (!status && !error) return null;

  if (!open) {
    return (
      <button
        type="button"
        onClick={() => setOpen(true)}
        style={{ ...panelStyle, width: "auto", padding: "9px 12px", cursor: "pointer", fontWeight: 800 }}
      >
        Charlemagne · {status?.version_label ?? "Configuration"}
      </button>
    );
  }

  return (
    <section style={panelStyle} aria-label="Mode de connexion Charlemagne">
      <div style={{ display: "flex", alignItems: "start", justifyContent: "space-between", gap: 10 }}>
        <div>
          <div style={{ fontSize: 11, fontWeight: 850, color: "#6c7889", textTransform: "uppercase", letterSpacing: ".06em" }}>
            Connexion Charlemagne
          </div>
          <strong style={{ display: "block", marginTop: 4, fontSize: 16 }}>{status?.version_label ?? "Configuration"}</strong>
        </div>
        <button type="button" onClick={() => setOpen(false)} style={{ ...buttonStyle, flex: "none", padding: "5px 8px" }}>Fermer</button>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr", gap: 7, marginTop: 14 }}>
        <button
          type="button"
          disabled={busy}
          onClick={() => void switchMode("import_file_v1")}
          style={{ ...buttonStyle, background: status?.mode === "import_file_v1" ? "#172033" : "white", color: status?.mode === "import_file_v1" ? "white" : "#445064" }}
        >V1 · Import</button>
        <button
          type="button"
          disabled={busy}
          onClick={() => void switchMode("sync_files_v2")}
          style={{ ...buttonStyle, background: status?.mode === "sync_files_v2" ? "#172033" : "white", color: status?.mode === "sync_files_v2" ? "white" : "#445064" }}
        >V2 · Exports</button>
        <button
          type="button"
          disabled={busy}
          onClick={() => void switchMode("api_v3")}
          style={{ ...buttonStyle, background: status?.mode === "api_v3" ? "#172033" : "white", color: status?.mode === "api_v3" ? "white" : "#445064" }}
        >V3 · API</button>
      </div>

      {status && (
        <div style={{ marginTop: 12, padding: 10, borderRadius: 9, background: "#f3f5f8", fontSize: 12, lineHeight: 1.45 }}>
          <strong>{status.transport_label}</strong>
          <div style={{ marginTop: 4, color: status.live_ready ? "#287244" : "#826000" }}>
            {status.live_ready ? "Mode disponible" : "Mode sécurisé : aucun envoi réel"}
          </div>
          {status.blocked_reason && <div style={{ marginTop: 6, color: "#667284" }}>{status.blocked_reason}</div>}
        </div>
      )}

      {status?.mode === "sync_files_v2" && (
        <div style={{ marginTop: 12 }}>
          <button type="button" disabled={busy} onClick={() => void importSyncFile()} style={{ ...buttonStyle, width: "100%" }}>
            {busy ? "Lecture…" : "Importer un export Charlemagne"}
          </button>
          <div style={{ marginTop: 6, color: "#6c7889", fontSize: 11 }}>Formats : PDF, CSV, TSV, TXT. Les données restent locales et sont d'abord mises en aperçu.</div>
        </div>
      )}

      {preview && (
        <div style={{ marginTop: 12, padding: 10, border: "1px solid #dfe4eb", borderRadius: 9, fontSize: 11, lineHeight: 1.4 }}>
          <strong>{preview.file_name}</strong>
          <div style={{ marginTop: 4 }}>{preview.line_count} ligne(s) · {preview.column_count || "?"} colonne(s){preview.separator ? ` · séparateur ${preview.separator}` : ""}</div>
          <div style={{ marginTop: 4, color: preview.duplicate ? "#826000" : "#287244" }}>{preview.duplicate ? "Export déjà importé : aucune duplication." : "Export enregistré pour mapping."}</div>
          {preview.headers.length > 0 && <div style={{ marginTop: 7, color: "#667284" }}>Colonnes détectées : {preview.headers.slice(0, 8).join(" · ")}</div>}
        </div>
      )}

      {error && <div style={{ marginTop: 10, color: "#9e2929", fontSize: 12 }}>{error}</div>}
    </section>
  );
}
