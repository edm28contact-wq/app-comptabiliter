import { useEffect, useMemo, useRef, useState } from "react";
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

type ColumnMapping = {
  date: number | null;
  journal: number | null;
  entry_number: number | null;
  account: number | null;
  account_label: number | null;
  aux_account: number | null;
  aux_label: number | null;
  piece: number | null;
  label: number | null;
  debit: number | null;
  credit: number | null;
  amount: number | null;
  direction: number | null;
  analytic_code: number | null;
  supplier: number | null;
  currency: number | null;
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
  duplicate_of: string | null;
  format_label: string;
  mapping: ColumnMapping;
  mapping_complete: boolean;
  warnings: string[];
};

type SyncCommitResult = {
  status: string;
  content_hash: string;
  imported_rows: number;
  updated_rows: number;
  skipped_rows: number;
  mirror_entries: number;
  inferred_supplier_rules: number;
  years: string[];
};

type SyncSummary = {
  folder: string | null;
  import_files: number;
  imported_files: number;
  pending_mapping: number;
  error_files: number;
  mirror_entries: number;
  accounts: number;
  suppliers: number;
  last_imported_at: string | null;
};

type SyncScanResult = {
  detected: number;
  imported: number;
  pending_mapping: number;
  duplicates: number;
  errors: number;
};

type SyncImportRecord = {
  path: string;
  file_name: string;
  kind: string;
  status: string;
  content_hash: string;
  line_count: number;
  column_count: number;
  separator: string | null;
  format_label: string | null;
  imported_rows: number;
  skipped_rows: number;
  error: string | null;
  imported_at: string;
  updated_at: string;
};

const emptyMapping: ColumnMapping = {
  date: null,
  journal: null,
  entry_number: null,
  account: null,
  account_label: null,
  aux_account: null,
  aux_label: null,
  piece: null,
  label: null,
  debit: null,
  credit: null,
  amount: null,
  direction: null,
  analytic_code: null,
  supplier: null,
  currency: null,
};

const panelStyle: React.CSSProperties = {
  position: "fixed",
  right: 18,
  bottom: 18,
  zIndex: 80,
  width: 430,
  maxHeight: "calc(100vh - 36px)",
  overflowY: "auto",
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

const smallText: React.CSSProperties = { fontSize: 11, color: "#667284", lineHeight: 1.4 };
const inputStyle: React.CSSProperties = {
  width: "100%",
  border: "1px solid #d7dde6",
  borderRadius: 7,
  padding: "6px 7px",
  background: "white",
  color: "#172033",
};

const mappingFields: Array<{ key: keyof ColumnMapping; label: string }> = [
  { key: "date", label: "Date *" },
  { key: "journal", label: "Journal" },
  { key: "entry_number", label: "N° écriture" },
  { key: "account", label: "Compte *" },
  { key: "account_label", label: "Libellé compte" },
  { key: "aux_account", label: "Compte auxiliaire" },
  { key: "aux_label", label: "Libellé auxiliaire" },
  { key: "piece", label: "Pièce / facture" },
  { key: "label", label: "Libellé écriture" },
  { key: "debit", label: "Débit *" },
  { key: "credit", label: "Crédit *" },
  { key: "amount", label: "Montant *" },
  { key: "direction", label: "Sens D/C *" },
  { key: "analytic_code", label: "Analytique" },
  { key: "supplier", label: "Fournisseur" },
  { key: "currency", label: "Devise" },
];

function statusLabel(status: string) {
  if (status === "importe") return "Synchronisé";
  if (status === "a_mapper") return "Mapping requis";
  if (status === "pret_a_importer") return "Prêt";
  if (status === "attente_stabilite") return "Copie en cours";
  if (status === "doublon") return "Doublon";
  if (status === "erreur") return "Erreur";
  return status;
}

export default function CharlemagneMode() {
  const [status, setStatus] = useState<ConnectorStatus | null>(null);
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [mapping, setMapping] = useState<ColumnMapping>(emptyMapping);
  const [summary, setSummary] = useState<SyncSummary | null>(null);
  const [imports, setImports] = useState<SyncImportRecord[]>([]);
  const [openPanel, setOpenPanel] = useState(false);
  const [busy, setBusy] = useState(false);
  const [replaceExisting, setReplaceExisting] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const scanBusy = useRef(false);

  const mappingComplete = useMemo(
    () =>
      mapping.date !== null &&
      mapping.account !== null &&
      ((mapping.debit !== null && mapping.credit !== null) ||
        (mapping.amount !== null && mapping.direction !== null)),
    [mapping],
  );

  const refreshSyncState = async () => {
    const [nextSummary, nextImports] = await Promise.all([
      invoke<SyncSummary>("get_charlemagne_sync_summary"),
      invoke<SyncImportRecord[]>("list_charlemagne_sync_imports"),
    ]);
    setSummary(nextSummary);
    setImports(nextImports);
  };

  useEffect(() => {
    void (async () => {
      try {
        const next = await invoke<ConnectorStatus>("get_charlemagne_connector_status");
        setStatus(next);
        await refreshSyncState();
      } catch (reason) {
        setError(String(reason));
      }
    })();
  }, []);

  const scanFolder = async (silent = false) => {
    if (scanBusy.current || status?.mode !== "sync_files_v2" || !summary?.folder) return;
    scanBusy.current = true;
    if (!silent) setBusy(true);
    try {
      const result = await invoke<SyncScanResult>("scan_charlemagne_sync_folder");
      await refreshSyncState();
      if (!silent || result.imported > 0 || result.errors > 0 || result.pending_mapping > 0) {
        setNotice(
          `${result.detected} fichier(s) détecté(s) · ${result.imported} synchronisé(s) · ${result.pending_mapping} à mapper · ${result.duplicates} doublon(s)`,
        );
      }
      if (result.imported > 0) {
        window.dispatchEvent(new Event("charlemagne-sync-updated"));
      }
    } catch (reason) {
      if (!silent) setError(String(reason));
    } finally {
      scanBusy.current = false;
      if (!silent) setBusy(false);
    }
  };

  useEffect(() => {
    if (status?.mode !== "sync_files_v2" || !summary?.folder) return;
    void scanFolder(true);
    const interval = window.setInterval(() => void scanFolder(true), 5000);
    return () => window.clearInterval(interval);
  }, [status?.mode, summary?.folder]);

  const switchMode = async (mode: "import_file_v1" | "sync_files_v2" | "api_v3") => {
    if (busy || status?.mode === mode) return;
    setBusy(true);
    setError(null);
    setPreview(null);
    setNotice(null);
    try {
      const next = await invoke<ConnectorStatus>("set_charlemagne_connector_mode", { mode });
      setStatus(next);
      await refreshSyncState();
      window.dispatchEvent(new Event("charlemagne-sync-updated"));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const chooseSyncFolder = async () => {
    const selected = await openDialog({ multiple: false, directory: true });
    if (!selected || Array.isArray(selected)) return;
    setBusy(true);
    setError(null);
    try {
      await invoke("set_charlemagne_sync_folder", { path: selected });
      await refreshSyncState();
      setNotice("Dossier Charlemagne configuré. La surveillance automatique est active.");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const analyzePath = async (path: string) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const result = await invoke<SyncPreview>("import_charlemagne_sync_file", { path });
      setPreview(result);
      setMapping(result.mapping);
      if (result.duplicate) {
        setNotice(`Ce contenu a déjà été détecté${result.duplicate_of ? ` dans ${result.duplicate_of}` : ""}.`);
      } else if (result.mapping_complete) {
        setNotice(`${result.format_label} reconnu automatiquement. Vérifiez puis synchronisez.`);
      } else {
        setNotice("Colonnes non reconnues à 100 %. Complétez le mapping ci-dessous.");
      }
      await refreshSyncState();
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
      filters: [
        { name: "Export comptable Charlemagne / FEC", extensions: ["fec", "txt", "csv", "tsv", "pdf"] },
      ],
    });
    if (!selected || Array.isArray(selected)) return;
    await analyzePath(selected);
  };

  const commitPreview = async () => {
    if (!preview || preview.duplicate || !mappingComplete || busy) return;
    setBusy(true);
    setError(null);
    try {
      const result = await invoke<SyncCommitResult>("commit_charlemagne_sync_file", {
        path: preview.path,
        mapping,
        replaceExisting,
      });
      setNotice(
        `Synchronisation terminée : ${result.imported_rows} nouvelle(s), ${result.updated_rows} mise(s) à jour, ${result.skipped_rows} ignorée(s). ${result.mirror_entries} lignes Charlemagne disponibles.`,
      );
      setPreview(null);
      setMapping(emptyMapping);
      await refreshSyncState();
      window.dispatchEvent(new Event("charlemagne-sync-updated"));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const updateMapping = (key: keyof ColumnMapping, value: string) => {
    setMapping((current) => ({
      ...current,
      [key]: value === "" ? null : Number.parseInt(value, 10),
    }));
  };

  if (!status && !error) return null;

  if (!openPanel) {
    return (
      <button
        type="button"
        onClick={() => setOpenPanel(true)}
        style={{ ...panelStyle, width: "auto", maxHeight: "none", padding: "9px 12px", cursor: "pointer", fontWeight: 800 }}
      >
        Charlemagne · {status?.version_label ?? "Configuration"}
        {status?.mode === "sync_files_v2" && summary ? ` · ${summary.mirror_entries} écritures` : ""}
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
        <button type="button" onClick={() => setOpenPanel(false)} style={{ ...buttonStyle, flex: "none", padding: "5px 8px" }}>Fermer</button>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr", gap: 7, marginTop: 14 }}>
        <button type="button" disabled={busy} onClick={() => void switchMode("import_file_v1")} style={{ ...buttonStyle, background: status?.mode === "import_file_v1" ? "#172033" : "white", color: status?.mode === "import_file_v1" ? "white" : "#445064" }}>V1 · Import</button>
        <button type="button" disabled={busy} onClick={() => void switchMode("sync_files_v2")} style={{ ...buttonStyle, background: status?.mode === "sync_files_v2" ? "#172033" : "white", color: status?.mode === "sync_files_v2" ? "white" : "#445064" }}>V2 · Exports</button>
        <button type="button" disabled={busy} onClick={() => void switchMode("api_v3")} style={{ ...buttonStyle, background: status?.mode === "api_v3" ? "#172033" : "white", color: status?.mode === "api_v3" ? "white" : "#445064" }}>V3 · API</button>
      </div>

      {status && (
        <div style={{ marginTop: 12, padding: 10, borderRadius: 9, background: "#f3f5f8", fontSize: 12, lineHeight: 1.45 }}>
          <strong>{status.transport_label}</strong>
          <div style={{ marginTop: 4, color: status.live_ready ? "#287244" : "#826000" }}>
            {status.live_ready ? "Mode opérationnel" : "Mode sécurisé : aucun envoi réel"}
          </div>
          {status.blocked_reason && <div style={{ marginTop: 6, color: "#667284" }}>{status.blocked_reason}</div>}
        </div>
      )}

      {status?.mode === "sync_files_v2" && (
        <>
          <div style={{ marginTop: 12, padding: 10, border: "1px solid #dfe4eb", borderRadius: 9 }}>
            <strong style={{ fontSize: 12 }}>Synchronisation automatique</strong>
            <div style={{ ...smallText, marginTop: 4, wordBreak: "break-all" }}>
              {summary?.folder ?? "Aucun dossier configuré"}
            </div>
            <div style={{ display: "flex", gap: 7, marginTop: 8 }}>
              <button type="button" disabled={busy} onClick={() => void chooseSyncFolder()} style={buttonStyle}>Choisir le dossier</button>
              <button type="button" disabled={busy || !summary?.folder} onClick={() => void scanFolder(false)} style={buttonStyle}>Synchroniser</button>
            </div>
            <div style={{ ...smallText, marginTop: 7 }}>
              Le dossier est vérifié toutes les 5 s. Un fichier doit être stable sur deux lectures avant traitement.
            </div>
          </div>

          {summary && (
            <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 6, marginTop: 9 }}>
              {[
                ["Écritures", summary.mirror_entries],
                ["Comptes", summary.accounts],
                ["Fournisseurs", summary.suppliers],
                ["Imports", summary.imported_files],
                ["À mapper", summary.pending_mapping],
                ["Erreurs", summary.error_files],
              ].map(([label, value]) => (
                <div key={String(label)} style={{ padding: 7, background: "#f6f7f9", borderRadius: 7 }}>
                  <strong style={{ display: "block", fontSize: 14 }}>{value}</strong>
                  <span style={{ fontSize: 10, color: "#6c7889" }}>{label}</span>
                </div>
              ))}
            </div>
          )}

          <div style={{ display: "flex", gap: 7, marginTop: 10 }}>
            <button type="button" disabled={busy} onClick={() => void importSyncFile()} style={buttonStyle}>
              {busy ? "Traitement…" : "Importer un fichier"}
            </button>
          </div>
          <div style={{ ...smallText, marginTop: 5 }}>
            Recommandé : FEC ou TXT/CSV/TSV. PDF accepté pour lecture, mais un export tabulaire est nécessaire pour une synchronisation fiable.
          </div>
        </>
      )}

      {preview && (
        <div style={{ marginTop: 12, padding: 10, border: "1px solid #dfe4eb", borderRadius: 9, fontSize: 11, lineHeight: 1.4 }}>
          <strong>{preview.file_name}</strong>
          <div style={{ marginTop: 4 }}>{preview.format_label} · {preview.line_count} ligne(s) · {preview.column_count || "?"} colonne(s){preview.separator ? ` · ${preview.separator}` : ""}</div>
          <div style={{ marginTop: 4, color: preview.duplicate ? "#826000" : preview.mapping_complete ? "#287244" : "#826000" }}>
            {preview.duplicate ? "Doublon détecté : aucune réimportation." : preview.mapping_complete ? "Mapping automatique complet." : "Mapping manuel nécessaire."}
          </div>
          {preview.warnings.map((warning) => <div key={warning} style={{ marginTop: 5, color: "#826000" }}>{warning}</div>)}

          {!preview.duplicate && preview.headers.length > 0 && (
            <details open={!preview.mapping_complete} style={{ marginTop: 9 }}>
              <summary style={{ cursor: "pointer", fontWeight: 800 }}>Correspondance des colonnes</summary>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 6, marginTop: 7 }}>
                {mappingFields.map((field) => (
                  <label key={field.key} style={{ fontSize: 10, color: "#667284" }}>
                    {field.label}
                    <select value={mapping[field.key] ?? ""} onChange={(event) => updateMapping(field.key, event.target.value)} style={{ ...inputStyle, marginTop: 3 }}>
                      <option value="">—</option>
                      {preview.headers.map((header, index) => <option key={`${field.key}-${index}`} value={index}>{header || `Colonne ${index + 1}`}</option>)}
                    </select>
                  </label>
                ))}
              </div>
              <div style={{ ...smallText, marginTop: 7 }}>* Obligatoire : Date + Compte + (Débit et Crédit) ou (Montant et Sens).</div>
            </details>
          )}

          {!preview.duplicate && (
            <>
              <label style={{ display: "flex", gap: 7, alignItems: "start", marginTop: 9, fontSize: 10, color: "#667284" }}>
                <input type="checkbox" checked={replaceExisting} onChange={(event) => setReplaceExisting(event.target.checked)} />
                Remplacer les écritures déjà synchronisées pour le ou les exercices présents dans ce fichier. À utiliser seulement avec un export complet de l'exercice.
              </label>
              <button type="button" disabled={busy || !mappingComplete} onClick={() => void commitPreview()} style={{ ...buttonStyle, width: "100%", marginTop: 9, opacity: mappingComplete ? 1 : 0.5 }}>
                Synchroniser dans l'application
              </button>
            </>
          )}
        </div>
      )}

      {imports.length > 0 && status?.mode === "sync_files_v2" && (
        <details style={{ marginTop: 10 }}>
          <summary style={{ cursor: "pointer", fontSize: 11, fontWeight: 800 }}>Historique des exports ({imports.length})</summary>
          <div style={{ display: "grid", gap: 5, marginTop: 6 }}>
            {imports.slice(0, 8).map((item) => (
              <button key={item.path} type="button" onClick={() => void analyzePath(item.path)} style={{ ...buttonStyle, textAlign: "left", fontSize: 10, padding: 7 }}>
                <span style={{ display: "block" }}>{item.file_name}</span>
                <span style={{ color: item.status === "erreur" ? "#9e2929" : "#667284", fontWeight: 500 }}>
                  {statusLabel(item.status)} · {item.imported_rows} ligne(s){item.error ? ` · ${item.error}` : ""}
                </span>
              </button>
            ))}
          </div>
        </details>
      )}

      {notice && <div style={{ marginTop: 10, padding: 8, borderRadius: 8, background: "#eef6f0", color: "#315f42", fontSize: 11 }}>{notice}</div>}
      {error && <div style={{ marginTop: 10, padding: 8, borderRadius: 8, background: "#fff0f0", color: "#9e2929", fontSize: 11 }}>{error}</div>}
    </section>
  );
}
