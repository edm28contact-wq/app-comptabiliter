import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type ConnectorStatus = {
  mode: string;
  version_label: string;
  transport_label: string;
  live_ready: boolean;
  preparation_ready: boolean;
  blocked_reason: string | null;
  switch_available: boolean;
};

const panelStyle: React.CSSProperties = {
  position: "fixed",
  right: 18,
  bottom: 18,
  zIndex: 80,
  width: 310,
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
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void invoke<ConnectorStatus>("get_charlemagne_connector_status")
      .then(setStatus)
      .catch((reason) => setError(String(reason)));
  }, []);

  const switchMode = async (mode: "import_file_v1" | "api_v2") => {
    if (busy || status?.mode === mode) return;
    setBusy(true);
    setError(null);
    try {
      const next = await invoke<ConnectorStatus>("set_charlemagne_connector_mode", { mode });
      setStatus(next);
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
        style={{
          ...panelStyle,
          width: "auto",
          padding: "9px 12px",
          cursor: "pointer",
          fontWeight: 800,
        }}
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
          <strong style={{ display: "block", marginTop: 4, fontSize: 16 }}>
            {status?.version_label ?? "Configuration"}
          </strong>
        </div>
        <button type="button" onClick={() => setOpen(false)} style={{ ...buttonStyle, flex: "none", padding: "5px 8px" }}>
          Fermer
        </button>
      </div>

      <div style={{ display: "flex", gap: 8, marginTop: 14 }}>
        <button
          type="button"
          disabled={busy}
          onClick={() => void switchMode("import_file_v1")}
          style={{
            ...buttonStyle,
            background: status?.mode === "import_file_v1" ? "#172033" : "white",
            color: status?.mode === "import_file_v1" ? "white" : "#445064",
          }}
        >
          V1 · Fichier
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => void switchMode("api_v2")}
          style={{
            ...buttonStyle,
            background: status?.mode === "api_v2" ? "#172033" : "white",
            color: status?.mode === "api_v2" ? "white" : "#445064",
          }}
        >
          V2 · API
        </button>
      </div>

      {status && (
        <div style={{ marginTop: 12, padding: 10, borderRadius: 9, background: "#f3f5f8", fontSize: 12, lineHeight: 1.45 }}>
          <strong>{status.transport_label}</strong>
          <div style={{ marginTop: 4, color: status.live_ready ? "#287244" : "#826000" }}>
            {status.live_ready ? "Connexion réelle disponible" : "Mode sécurisé : aucun envoi réel"}
          </div>
          {status.blocked_reason && <div style={{ marginTop: 6, color: "#667284" }}>{status.blocked_reason}</div>}
        </div>
      )}

      {error && <div style={{ marginTop: 10, color: "#9e2929", fontSize: 12 }}>{error}</div>}
    </section>
  );
}
