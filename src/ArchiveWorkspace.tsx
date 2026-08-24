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
  charlemagne_status: string;
};

type ArchiveRule = {
  supplier: string;
  archive_folder: string;
  use_count: number;
  updated_at: string;
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

export default function ArchiveWorkspace() {
  const [openWorkspace, setOpenWorkspace] = useState(false);
  const [items, setItems] = useState<ArchiveItem[]>([]);
  const [rules, setRules] = useState<ArchiveRule[]>([]);
  const [filter, setFilter] = useState<Filter>("todo");
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const refresh = async () => {
    const [documents, learnedRules] = await Promise.all([
      invoke<ArchiveItem[]>("list_archive_workspace"),
      invoke<ArchiveRule[]>("list_archive_rules"),
    ]);
    setItems(documents);
    setRules(learnedRules);
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

  const chooseDestination = async (item: ArchiveItem) => {
    const folder = await open({ multiple: false, directory: true });
    if (!folder || Array.isArray(folder)) return;
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
                          {item.target_folder && !item.archive_path && <small style={{ display: "block", opacity: .6 }}>Proposition : {item.target_source}</small>}
                        </td>
                        <td style={{ padding: 10 }}>
                          {classification(item) === "done" ? "Classée" : classification(item) === "errors" ? "Erreur" : item.status === "validee" ? "Validée" : "À traiter"}
                          {item.archive_error && <small style={{ display: "block", maxWidth: 260 }}>{item.archive_error}</small>}
                        </td>
                        <td style={{ padding: 10 }}>
                          <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                            {!item.archive_path && (item.status === "validee" || item.status === "archive_erreur") && (
                              <button disabled={busy === item.path} type="button" onClick={() => void chooseDestination(item)}>Choisir dossier</button>
                            )}
                            {!item.archive_path && item.target_folder && item.status === "validee" && (
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
