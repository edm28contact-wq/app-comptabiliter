import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type ConnectorStatus = {
  mode: string;
};

type Candidate = {
  path: string;
  file_name: string;
  charlemagne_status: string;
  archive_path: string | null;
  prepared_at: string | null;
};

type TxtProfile = {
  journal: string;
  debit_marker: string;
  credit_marker: string;
  decimal_separator: string;
  analytic_label: string | null;
  specification_confirmed: boolean;
};

type TxtPreview = {
  format_label: string;
  column_count: number;
  line_count: number;
  separator: string;
  content: string;
  rows: string[][];
  production_ready: boolean;
  warnings: string[];
};

const STORAGE_KEY = "app-comptabiliter:charlemagne-txt-profile";

const defaultProfile: TxtProfile = {
  journal: "ACH",
  debit_marker: "D",
  credit_marker: "C",
  decimal_separator: ".",
  analytic_label: null,
  specification_confirmed: false,
};

const panelStyle: React.CSSProperties = {
  position: "fixed",
  left: 18,
  bottom: 18,
  zIndex: 79,
  width: 760,
  maxWidth: "calc(100vw - 36px)",
  maxHeight: "calc(100vh - 36px)",
  overflow: "auto",
  padding: 14,
  border: "1px solid #dfe4eb",
  borderRadius: 14,
  background: "#fff",
  boxShadow: "0 14px 40px rgba(20,31,50,.16)",
  color: "#172033",
  fontFamily: "Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
};

const inputStyle: React.CSSProperties = {
  width: "100%",
  border: "1px solid #d7dde6",
  borderRadius: 7,
  padding: "7px 8px",
  background: "white",
  color: "#172033",
};

const buttonStyle: React.CSSProperties = {
  border: "1px solid #d7dde6",
  borderRadius: 9,
  padding: "8px 10px",
  background: "white",
  color: "#445064",
  fontWeight: 800,
  cursor: "pointer",
};

function loadStoredProfile(): TxtProfile {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return defaultProfile;
    const parsed = JSON.parse(raw) as Partial<TxtProfile>;
    return {
      ...defaultProfile,
      ...parsed,
      specification_confirmed: false,
    };
  } catch {
    return defaultProfile;
  }
}

function saveProfile(profile: TxtProfile) {
  try {
    window.localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ ...profile, specification_confirmed: false }),
    );
  } catch {
    // Une préférence locale ne doit jamais bloquer le flux comptable.
  }
}

export default function CharlemagneTxtPreview() {
  const [enabled, setEnabled] = useState(false);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const [selectedPath, setSelectedPath] = useState("");
  const [profile, setProfile] = useState<TxtProfile>(loadStoredProfile);
  const [preview, setPreview] = useState<TxtPreview | null>(null);
  const [error, setError] = useState<string | null>(null);

  const selected = useMemo(
    () => candidates.find((candidate) => candidate.path === selectedPath) ?? null,
    [candidates, selectedPath],
  );

  const refresh = async () => {
    try {
      const [status, nextCandidates] = await Promise.all([
        invoke<ConnectorStatus>("get_charlemagne_connector_status"),
        invoke<Candidate[]>("list_charlemagne_txt_candidates"),
      ]);
      const isEnabled = status.mode === "import_file_v1";
      setEnabled(isEnabled);
      setCandidates(nextCandidates);
      if (!isEnabled) {
        setOpen(false);
        setPreview(null);
      }
      setSelectedPath((current) => {
        if (current && nextCandidates.some((candidate) => candidate.path === current)) {
          return current;
        }
        return nextCandidates[0]?.path ?? "";
      });
    } catch (reason) {
      setError(String(reason));
    }
  };

  useEffect(() => {
    void refresh();
    const refreshHandler = () => void refresh();
    window.addEventListener("charlemagne-sync-updated", refreshHandler);
    window.addEventListener("invoice-reading-updated", refreshHandler);
    return () => {
      window.removeEventListener("charlemagne-sync-updated", refreshHandler);
      window.removeEventListener("invoice-reading-updated", refreshHandler);
    };
  }, []);

  useEffect(() => saveProfile(profile), [profile]);

  const generatePreview = async () => {
    if (!selectedPath || busy) return;
    setBusy(true);
    setError(null);
    setPreview(null);
    try {
      const result = await invoke<TxtPreview>("preview_charlemagne_import_txt", {
        invoicePath: selectedPath,
        profile: { ...profile, specification_confirmed: false },
      });
      setPreview(result);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  if (!enabled) return null;

  if (!open) {
    return (
      <button
        type="button"
        onClick={() => setOpen(true)}
        style={{ ...panelStyle, width: "auto", maxHeight: "none", padding: "9px 12px", cursor: "pointer", fontWeight: 800 }}
      >
        Aperçu TXT · {candidates.length} facture(s)
      </button>
    );
  }

  return (
    <section style={panelStyle} aria-label="Aperçu TXT Charlemagne">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "start", gap: 12 }}>
        <div>
          <div style={{ fontSize: 11, fontWeight: 850, color: "#6c7889", textTransform: "uppercase", letterSpacing: ".06em" }}>
            Charlemagne V1
          </div>
          <strong style={{ display: "block", marginTop: 4, fontSize: 16 }}>
            Aperçu TXT provisoire
          </strong>
        </div>
        <button type="button" style={buttonStyle} onClick={() => setOpen(false)}>Fermer</button>
      </div>

      <div style={{ marginTop: 12, padding: 9, borderRadius: 8, background: "#fff6dc", color: "#715700", fontSize: 11, lineHeight: 1.45 }}>
        Aucun export réel n'est disponible ici. L'ordre des 10 colonnes, les marqueurs D/C, le séparateur décimal et l'encodage final doivent encore être confirmés dans la documentation APLIM.
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "2fr 1fr 1fr 1fr 1.4fr", gap: 7, marginTop: 12 }}>
        <label style={{ fontSize: 10, color: "#667284" }}>
          Facture préparée
          <select value={selectedPath} onChange={(event) => { setSelectedPath(event.target.value); setPreview(null); }} style={{ ...inputStyle, marginTop: 3 }}>
            {candidates.length === 0 && <option value="">Aucune facture prête</option>}
            {candidates.map((candidate) => (
              <option key={candidate.path} value={candidate.path}>
                {candidate.file_name} · {candidate.charlemagne_status}
              </option>
            ))}
          </select>
        </label>
        <label style={{ fontSize: 10, color: "#667284" }}>
          Journal
          <input value={profile.journal} onChange={(event) => setProfile((current) => ({ ...current, journal: event.target.value }))} style={{ ...inputStyle, marginTop: 3 }} />
        </label>
        <label style={{ fontSize: 10, color: "#667284" }}>
          Débit
          <input value={profile.debit_marker} onChange={(event) => setProfile((current) => ({ ...current, debit_marker: event.target.value }))} style={{ ...inputStyle, marginTop: 3 }} />
        </label>
        <label style={{ fontSize: 10, color: "#667284" }}>
          Crédit
          <input value={profile.credit_marker} onChange={(event) => setProfile((current) => ({ ...current, credit_marker: event.target.value }))} style={{ ...inputStyle, marginTop: 3 }} />
        </label>
        <label style={{ fontSize: 10, color: "#667284" }}>
          Libellé analytique
          <input value={profile.analytic_label ?? ""} onChange={(event) => setProfile((current) => ({ ...current, analytic_label: event.target.value || null }))} style={{ ...inputStyle, marginTop: 3 }} />
        </label>
      </div>

      <div style={{ display: "flex", alignItems: "end", gap: 8, marginTop: 8 }}>
        <label style={{ width: 150, fontSize: 10, color: "#667284" }}>
          Décimales
          <select value={profile.decimal_separator} onChange={(event) => setProfile((current) => ({ ...current, decimal_separator: event.target.value }))} style={{ ...inputStyle, marginTop: 3 }}>
            <option value=".">Point (1234.56)</option>
            <option value=",">Virgule (1234,56)</option>
          </select>
        </label>
        <button type="button" disabled={busy || !selectedPath} onClick={() => void generatePreview()} style={{ ...buttonStyle, opacity: selectedPath ? 1 : 0.5 }}>
          {busy ? "Génération…" : "Générer l'aperçu"}
        </button>
        {selected && (
          <span style={{ fontSize: 10, color: "#667284", wordBreak: "break-all" }}>
            {selected.archive_path ?? selected.path}
          </span>
        )}
      </div>

      {preview && (
        <div style={{ marginTop: 12 }}>
          <div style={{ display: "flex", gap: 12, alignItems: "center", fontSize: 11 }}>
            <strong>{preview.format_label}</strong>
            <span>{preview.line_count} ligne(s)</span>
            <span>{preview.column_count} colonnes</span>
            <span>Séparateur : {preview.separator}</span>
            <span style={{ color: preview.production_ready ? "#287244" : "#9b6a00", fontWeight: 800 }}>
              {preview.production_ready ? "Production autorisée" : "Production bloquée"}
            </span>
          </div>

          {preview.warnings.map((warning) => (
            <div key={warning} style={{ marginTop: 5, color: "#826000", fontSize: 10 }}>{warning}</div>
          ))}

          <div style={{ overflowX: "auto", marginTop: 9, border: "1px solid #e1e5ea", borderRadius: 8 }}>
            <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 10, whiteSpace: "nowrap" }}>
              <thead>
                <tr style={{ background: "#f5f7f9" }}>
                  {["Date", "Journal", "Compte", "Libellé compte", "Pièce", "Libellé opération", "Montant", "Sens", "Analytique", "Libellé analytique"].map((header) => (
                    <th key={header} style={{ padding: "6px 7px", textAlign: "left", borderBottom: "1px solid #e1e5ea" }}>{header}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {preview.rows.map((row, rowIndex) => (
                  <tr key={`${rowIndex}-${row.join("|")}`}>
                    {row.map((value, columnIndex) => (
                      <td key={`${rowIndex}-${columnIndex}`} style={{ padding: "6px 7px", borderBottom: "1px solid #eef1f4" }}>{value || "—"}</td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <details style={{ marginTop: 9 }}>
            <summary style={{ cursor: "pointer", fontSize: 10, fontWeight: 800 }}>Voir le TXT brut</summary>
            <pre style={{ marginTop: 6, padding: 8, maxHeight: 180, overflow: "auto", background: "#f6f7f9", borderRadius: 7, fontSize: 9, whiteSpace: "pre" }}>
              {preview.content}
            </pre>
          </details>
        </div>
      )}

      {error && <div style={{ marginTop: 10, padding: 8, borderRadius: 8, background: "#fff0f0", color: "#9e2929", fontSize: 11 }}>{error}</div>}
    </section>
  );
}
