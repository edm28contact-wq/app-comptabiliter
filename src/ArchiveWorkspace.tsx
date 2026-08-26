import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

type ArchiveItem = {
  path: string;
  file_name: string;
  status: string;
  archive_path: string | null;
  archive_error: string | null;
  archived_at: string | null;
  supplier: string | null;
  invoice_number: string | null;
  invoice_date: string | null;
  amount_ttc: string | null;
  confidence: number;
  target_folder: string | null;
  target_source: string;
  target_confidence: number;
  charlemagne_status: string;
};

type ArchiveRule = {
  supplier: string;
  archive_folder: string;
  use_count: number;
  updated_at: string;
};

type ArchiveScanResult = {
  root: string;
  folders_scanned: number;
  truncated: boolean;
};

type Filter = "todo" | "done" | "errors" | "all";

const formatAmount = (value: string | null) => {
  if (!value) return "—";
  const amount = Number.parseFloat(value.replace(",", "."));
  if (!Number.isFinite(amount)) return value;
  return new Intl.NumberFormat("fr-FR", { style: "currency", currency: "EUR" }).format(amount);
};

const classification = (item: ArchiveItem): Filter => {
  if (item.status === "archive_erreur" || item.archive_error) return "errors";
  if (item.status === "classee" || Boolean(item.archive_path)) return "done";
  return "todo";
};

const targetLabel = (item: ArchiveItem) => {
  if (item.target_source === "arborescence_existante") return `Dossier repéré · ${item.target_confidence}%`;
  if (item.target_source === "classement_manuel") return "Choisi manuellement";
  if (item.target_source === "memoire_fournisseur") return "Mémorisé fournisseur";
  if (item.target_source === "validation") return "Choisi à la validation";
  return item.target_source;
};

export default function ArchiveWorkspace() {
  const [openWorkspace, setOpenWorkspace] = useState(false);
  const [items, setItems] = useState<ArchiveItem[]>([]);
  const [rules, setRules] = useState<ArchiveRule[]>([]);
  const [archiveRoot, setArchiveRoot] = useState<string | null>(null);
  const [scanInfo, setScanInfo] = useState<ArchiveScanResult | null>(null);
  const [filter, setFilter] = useState<Filter>("todo");
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [rootBusy, setRootBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  const refresh = async () => {
    const [documents, learnedRules, root] = await Promise.all([
      invoke<ArchiveItem[]>("list_archive_workspace"),
      invoke<ArchiveRule[]>("list_archive_rules"),
      invoke<string | null>("get_archive_root"),
    ]);
    setItems(documents);
    setRules(learnedRules);
    setArchiveRoot(root);
  };

  useEffect(() => {
    if (!openWorkspace) return;
    void refresh().catch((error) => setMessage(String(error)));
  }, [openWorkspace]);

  const counts = useMemo(
    () => ({
      todo: items.filter((item) => classification(item) === "todo").length,
      done: items.filter((item) => classification(item) === "done").length,
      errors: items.filter((item) => classification(item) === "errors").length,
      all: items.length,
    }),
    [items],
  );

  const visible = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase("fr");
    return items.filter((item) => {
      if (filter !== "all" && classification(item) !== filter) return false;
      if (!normalizedQuery) return true;
      return [
        item.file_name,
        item.supplier ?? "",
        item.invoice_number ?? "",
        item.invoice_date ?? "",
        item.archive_path ?? "",
        item.target_folder ?? "",
      ].some((value) => value.toLocaleLowerCase("fr").includes(normalizedQuery));
    });
  }, [filter, items, query]);

  const chooseArchiveRoot = async () => {
    const folder = await open({ multiple: false, directory: true });
    if (!folder || Array.isArray(folder)) return;
    setRootBusy(true);
    try {
      const result = await invoke<ArchiveScanResult>("set_archive_root", { path: folder });
      setArchiveRoot(result.root);
      setScanInfo(result);
      setMessage(
        result.truncated
          ? `${result.folders_scanned} dossiers indexés. Limite de sécurité atteinte : l'arborescence est très volumineuse.`
          : `${result.folders_scanned} dossiers indexés en lecture seule.`,
      );
      await refresh();
    } catch (error) {
      setMessage(String(error));
    } finally {
      setRootBusy(false);
    }
  };

  const rescanArchiveTree = async () => {
    setRootBusy(true);
    try {
      const result = await invoke<ArchiveScanResult>("scan_archive_tree");
      setScanInfo(result);
      setMessage(
        result.truncated
          ? `${result.folders_scanned} dossiers réindexés. Limite de sécurité atteinte.`
          : `${result.folders_scanned} dossiers réindexés sans modifier les archives.`,
      );
      await refresh();
    } catch (error) {
      setMessage(String(error));
    } finally {
      setRootBusy(false);
    }
  };

  const saveDestination = async (item: ArchiveItem, folder: string) => {
    setBusy(item.path);
    try {
      await invoke("set_invoice_archive_destination", {
        path: item.path,
        folder,
        rememberSupplier: true,
      });
      setMessage(`Destination mémorisée${item.supplier ? ` pour ${item.supplier}` : ""}.`);
      await refresh();
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusy(null);
    }
  };

  const chooseDestination = async (item: ArchiveItem) => {
    const folder = await open({ multiple: false, directory: true });
    if (!folder || Array.isArray(folder)) return;
    await saveDestination(item, folder);
  };

  const acceptSuggestion = async (item: ArchiveItem) => {
    if (!item.target_folder || item.target_source !== "arborescence_existante") return;
    await saveDestination(item, item.target_folder);
  };

  const archiveNow = async (item: ArchiveItem) => {
    setBusy(item.path);
    try {
      await invoke("archive_invoice", { path: item.path });
      setMessage("Archive copiée, vérifiée par SHA-256 et enregistrée.");
      await refresh();
      window.dispatchEvent(new Event("invoice-reading-updated"));
    } catch (error) {
      setMessage(String(error));
      await refresh();
    } finally {
      setBusy(null);
    }
  };

  return (
    <>
      <button
        type="button"
        onClick={() => setOpenWorkspace(true)}
        style={{
          position: "fixed",
          right: 24,
          bottom: 24,
          zIndex: 900,
          border: 0,
          borderRadius: 12,
          padding: "12px 18px",
          fontWeight: 700,
          cursor: "pointer",
          boxShadow: "0 8px 30px rgba(0,0,0,.18)",
        }}
      >
        Classement · {counts.todo}
      </button>

      {openWorkspace && (
        <div
          style={{
            position: "fixed",
            inset: 0,
            zIndex: 1200,
            background: "#f5f7fb",
            color: "#182235",
            overflow: "auto",
          }}
        >
          <header style={{ display: "flex", justifyContent: "space-between", gap: 16, padding: "22px 28px", background: "white", borderBottom: "1px solid #dde3ec", position: "sticky", top: 0, zIndex: 2 }}>
            <div>
              <div style={{ fontSize: 12, textTransform: "uppercase", letterSpacing: ".08em", opacity: .65 }}>Documents validés et archives</div>
              <h1 style={{ margin: "5px 0 0" }}>Classement / Archives</h1>
            </div>
            <button type="button" onClick={() => setOpenWorkspace(false)} style={{ height: 40, padding: "0 16px" }}>Fermer</button>
          </header>

          <main style={{ maxWidth: 1500, margin: "0 auto", padding: 28 }}>
            {message && (
              <div style={{ padding: 14, marginBottom: 18, border: "1px solid #cbd5e1", background: "white", borderRadius: 10, display: "flex", justifyContent: "space-between", gap: 12 }}>
                <span>{message}</span><button type="button" onClick={() => setMessage(null)}>OK</button>
              </div>
            )}

            <section style={{ background: "white", border: "1px solid #d7dee8", borderRadius: 12, padding: 18, marginBottom: 18 }}>
              <div style={{ display: "flex", justifyContent: "space-between", gap: 16, alignItems: "center", flexWrap: "wrap" }}>
                <div style={{ minWidth: 260, flex: 1 }}>
                  <div style={{ fontSize: 12, textTransform: "uppercase", letterSpacing: ".06em", opacity: .65 }}>Arborescence existante</div>
                  <strong style={{ display: "block", marginTop: 5, overflowWrap: "anywhere" }}>{archiveRoot ?? "Aucune racine d'archives configurée"}</strong>
                  <small style={{ display: "block", marginTop: 5, opacity: .68 }}>
                    Analyse en lecture seule : l'app repère les dossiers existants et propose une destination sans créer ni déplacer quoi que ce soit automatiquement.
                  </small>
                  {scanInfo && <small style={{ display: "block", marginTop: 4 }}>{scanInfo.folders_scanned} dossier(s) indexé(s){scanInfo.truncated ? " · limite de sécurité atteinte" : ""}</small>}
                </div>
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                  <button disabled={rootBusy} type="button" onClick={() => void chooseArchiveRoot()}>{archiveRoot ? "Changer la racine" : "Choisir les archives"}</button>
                  {archiveRoot && <button disabled={rootBusy} type="button" onClick={() => void rescanArchiveTree()}>{rootBusy ? "Analyse…" : "Réindexer"}</button>}
                </div>
              </div>
            </section>

            <section style={{ display: "grid", gridTemplateColumns: "repeat(4,minmax(0,1fr))", gap: 12, marginBottom: 18 }}>
              {(["todo", "done", "errors", "all"] as Filter[]).map((name) => {
                const labels: Record<Filter, string> = { todo: "À classer", done: "Classées", errors: "Erreurs", all: "Toutes" };
                return (
                  <button key={name} type="button" onClick={() => setFilter(name)} style={{ textAlign: "left", padding: 18, borderRadius: 12, border: filter === name ? "2px solid #182235" : "1px solid #d7dee8", background: "white" }}>
                    <span style={{ display: "block", opacity: .7 }}>{labels[name]}</span>
                    <strong style={{ fontSize: 28 }}>{counts[name]}</strong>
                  </button>
                );
              })}
            </section>

            <section style={{ background: "white", border: "1px solid #d7dee8", borderRadius: 12, padding: 18 }}>
              <div style={{ display: "flex", justifyContent: "space-between", gap: 12, marginBottom: 16 }}>
                <input
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Rechercher fournisseur, facture, date, dossier…"
                  style={{ flex: 1, minWidth: 240, padding: "10px 12px" }}
                />
                <button type="button" onClick={() => void refresh()}>Actualiser</button>
              </div>

              <div style={{ overflowX: "auto" }}>
                <table style={{ width: "100%", borderCollapse: "collapse" }}>
                  <thead>
                    <tr style={{ textAlign: "left", borderBottom: "1px solid #d7dee8" }}>
                      <th style={{ padding: 10 }}>Fournisseur</th>
                      <th style={{ padding: 10 }}>Facture</th>
                      <th style={{ padding: 10 }}>Date</th>
                      <th style={{ padding: 10 }}>TTC</th>
                      <th style={{ padding: 10 }}>Lecture</th>
                      <th style={{ padding: 10 }}>Destination</th>
                      <th style={{ padding: 10 }}>État</th>
                      <th style={{ padding: 10 }}>Actions</th>
                    </tr>
                  </thead>
                  <tbody>
                    {visible.map((item) => (
                      <tr key={item.path} style={{ borderBottom: "1px solid #edf0f5", verticalAlign: "top" }}>
                        <td style={{ padding: 10 }}><strong>{item.supplier ?? "À identifier"}</strong><small style={{ display: "block", opacity: .6 }}>{item.file_name}</small></td>
                        <td style={{ padding: 10 }}>{item.invoice_number ?? "—"}</td>
                        <td style={{ padding: 10 }}>{item.invoice_date ?? "—"}</td>
                        <td style={{ padding: 10 }}>{formatAmount(item.amount_ttc)}</td>
                        <td style={{ padding: 10 }}><strong>{item.confidence}%</strong></td>
                        <td style={{ padding: 10, maxWidth: 360, overflowWrap: "anywhere" }}>
                          {item.archive_path ?? item.target_folder ?? "Aucune destination"}
                          {item.target_folder && !item.archive_path && <small style={{ display: "block", opacity: .6 }}>{targetLabel(item)}</small>}
                        </td>
                        <td style={{ padding: 10 }}>
                          {classification(item) === "done" ? "Classée" : classification(item) === "errors" ? "Erreur" : item.status === "validee" ? "Validée" : "À traiter"}
                          {item.archive_error && <small style={{ display: "block", maxWidth: 260 }}>{item.archive_error}</small>}
                        </td>
                        <td style={{ padding: 10 }}>
                          <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                            {!item.archive_path && item.target_folder && item.target_source === "arborescence_existante" && item.status === "validee" && (
                              <button disabled={busy === item.path} type="button" onClick={() => void acceptSuggestion(item)}>Accepter dossier proposé</button>
                            )}
                            {!item.archive_path && (item.status === "validee" || item.status === "archive_erreur") && (
                              <button disabled={busy === item.path} type="button" onClick={() => void chooseDestination(item)}>Choisir dossier</button>
                            )}
                            {!item.archive_path && item.target_folder && item.target_source !== "arborescence_existante" && item.status === "validee" && (
                              <button disabled={busy === item.path} type="button" onClick={() => void archiveNow(item)}>Classer maintenant</button>
                            )}
                            {(item.status === "archive_erreur" || item.status === "archive_source_presente") && (
                              <button disabled={busy === item.path} type="button" onClick={() => void archiveNow(item)}>Reprendre</button>
                            )}
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {visible.length === 0 && <div style={{ padding: 30, textAlign: "center", opacity: .65 }}>Aucun document dans cette vue.</div>}
              </div>
            </section>

            <section style={{ marginTop: 18, background: "white", border: "1px solid #d7dee8", borderRadius: 12, padding: 18 }}>
              <h2 style={{ marginTop: 0 }}>Dossiers mémorisés par fournisseur</h2>
              {rules.length === 0 ? <p>Aucune règle de classement mémorisée.</p> : (
                <div style={{ display: "grid", gap: 8 }}>
                  {rules.map((rule) => (
                    <div key={rule.supplier} style={{ display: "grid", gridTemplateColumns: "minmax(180px,1fr) 2fr auto", gap: 12, padding: 10, borderBottom: "1px solid #edf0f5" }}>
                      <strong>{rule.supplier}</strong>
                      <span style={{ overflowWrap: "anywhere" }}>{rule.archive_folder}</span>
                      <small>{rule.use_count} utilisation(s)</small>
                    </div>
                  ))}
                </div>
              )}
            </section>
          </main>
        </div>
      )}
    </>
  );
}
