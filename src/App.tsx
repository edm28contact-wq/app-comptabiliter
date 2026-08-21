import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";

type InvoiceRecord = {
  path: string;
  file_name: string;
  source: string;
  status: string;
  extraction_status: string;
  text_length: number;
};

type ParsedInvoice = {
  supplier: string | null;
  invoice_number: string | null;
  invoice_date: string | null;
  amount_ht: string | null;
  amount_vat: string | null;
  amount_ttc: string | null;
  siret: string | null;
  iban: string | null;
  amounts_consistent: boolean | null;
  confidence: number;
};

const isPdf = (path: string) => path.toLowerCase().endsWith(".pdf");

const extractionLabel = (status: string) => {
  if (status === "texte_extrait") return "Texte lu";
  if (status === "ocr_requis") return "OCR requis";
  return "À analyser";
};

function App() {
  const [files, setFiles] = useState<InvoiceRecord[]>([]);
  const [dragging, setDragging] = useState(false);
  const [watchedFolder, setWatchedFolder] = useState<string | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [selectedText, setSelectedText] = useState<string | null>(null);
  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [parsed, setParsed] = useState<ParsedInvoice | null>(null);

  const refreshInvoices = async () => {
    const records = await invoke<InvoiceRecord[]>("list_invoices");
    setFiles(records);
  };

  const registerPaths = async (paths: string[], source: string) => {
    const pdfs = paths.filter(isPdf);
    await Promise.all(pdfs.map((path) => invoke("register_invoice", { path, source })));
    await refreshInvoices();
  };

  const scanFolder = async (folder: string) => {
    try {
      await invoke<string[]>("scan_pdf_folder", { path: folder });
      await refreshInvoices();
      setFolderError(null);
    } catch (error) {
      setFolderError(String(error));
    }
  };

  useEffect(() => {
    const restore = async () => {
      try {
        const savedFolder = await invoke<string | null>("get_watched_folder");
        setWatchedFolder(savedFolder);
        await refreshInvoices();
      } catch (error) {
        setFolderError(String(error));
      }
    };
    void restore();
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWindow().onDragDropEvent((event) => {
      if (event.payload.type === "over") setDragging(true);
      if (event.payload.type === "leave") setDragging(false);
      if (event.payload.type === "drop") {
        setDragging(false);
        void registerPaths(event.payload.paths, "glisser-deposer");
      }
    }).then((fn) => { unlisten = fn; });
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    if (!watchedFolder) return;
    void scanFolder(watchedFolder);
    const intervalId = window.setInterval(() => { void scanFolder(watchedFolder); }, 2000);
    return () => window.clearInterval(intervalId);
  }, [watchedFolder]);

  const chooseFiles = async () => {
    const selected = await open({
      multiple: true,
      directory: false,
      filters: [{ name: "Factures PDF", extensions: ["pdf"] }]
    });
    if (!selected) return;
    await registerPaths(Array.isArray(selected) ? selected : [selected], "manuel");
  };

  const chooseFolder = async () => {
    const selected = await open({ multiple: false, directory: true });
    if (!selected || Array.isArray(selected)) return;
    try {
      await invoke("set_watched_folder", { path: selected });
      setWatchedFolder(selected);
      setFolderError(null);
    } catch (error) {
      setFolderError(String(error));
    }
  };

  const reanalyze = async (file: InvoiceRecord) => {
    await invoke("analyze_invoice", { path: file.path });
    await refreshInvoices();
  };

  const inspectInvoice = async (file: InvoiceRecord) => {
    const [text, data] = await Promise.all([
      invoke<string | null>("get_invoice_text", { path: file.path }),
      invoke<ParsedInvoice | null>("get_invoice_parsed", { path: file.path })
    ]);
    setSelectedName(file.file_name);
    setSelectedText(text ?? "Aucun texte extrait.");
    setParsed(data);
  };

  const pendingCount = files.filter((file) => file.status === "nouvelle").length;
  const validatedCount = files.filter((file) => file.status === "validee").length;
  const ocrCount = files.filter((file) => file.extraction_status === "ocr_requis").length;

  return (
    <main className="shell">
      <header className="topbar">
        <div>
          <p className="eyebrow">Assistant Charlemagne</p>
          <h1>Factures fournisseurs</h1>
        </div>
        <span className="status">V0.5 · Analyse structurée</span>
      </header>

      <section className="stats">
        <article><strong>{files.length}</strong><span>Factures enregistrées</span></article>
        <article><strong>{pendingCount}</strong><span>À vérifier</span></article>
        <article><strong>{ocrCount}</strong><span>OCR requis</span></article>
        <article><strong>{validatedCount}</strong><span>Validées</span></article>
      </section>

      <section className="folder-card">
        <div>
          <p className="eyebrow">Source automatique</p>
          <h2>Dossier Windows surveillé</h2>
          <p className="folder-path">{watchedFolder ?? "Aucun dossier connecté pour le moment."}</p>
          {folderError && <p className="error">{folderError}</p>}
        </div>
        <button type="button" onClick={chooseFolder}>{watchedFolder ? "Changer de dossier" : "Connecter un dossier"}</button>
      </section>

      <section className={`dropzone ${dragging ? "is-dragging" : ""}`}>
        <div className="drop-icon">PDF</div>
        <h2>Déposez vos factures ici</h2>
        <p>Glissez des PDF depuis Windows ou sélectionnez-les manuellement.</p>
        <button type="button" onClick={chooseFiles}>Ajouter des factures</button>
      </section>

      <section className="queue">
        <div className="section-heading">
          <h2>File de traitement</h2>
          <span>{files.length} document{files.length > 1 ? "s" : ""}</span>
        </div>
        {files.length === 0 ? <div className="empty">Aucune facture enregistrée.</div> : (
          <ul>
            {files.map((file) => (
              <li key={file.path}>
                <div className="file-info">
                  <strong>{file.file_name}</strong>
                  <small>{file.path} · source : {file.source}</small>
                  <div className="file-actions">
                    {file.extraction_status === "texte_extrait" && (
                      <button type="button" className="secondary" onClick={() => inspectInvoice(file)}>Contrôler</button>
                    )}
                    <button type="button" className="secondary" onClick={() => reanalyze(file)}>Réanalyser</button>
                  </div>
                </div>
                <div className="badges">
                  <span className={`extraction ${file.extraction_status}`}>
                    {extractionLabel(file.extraction_status)}{file.text_length > 0 ? ` · ${file.text_length} car.` : ""}
                  </span>
                  <span className="pending">{file.status}</span>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      {selectedText !== null && (
        <section className="review-panel">
          <div className="section-heading">
            <h2>Contrôle · {selectedName}</h2>
            <button type="button" className="secondary" onClick={() => { setSelectedText(null); setParsed(null); }}>Fermer</button>
          </div>
          <div className="review-grid">
            <div className="parsed-card">
              <div className="confidence">Confiance extraction : <strong>{parsed?.confidence ?? 0}%</strong></div>
              <dl>
                <div><dt>Fournisseur</dt><dd>{parsed?.supplier ?? "À vérifier"}</dd></div>
                <div><dt>N° facture</dt><dd>{parsed?.invoice_number ?? "À vérifier"}</dd></div>
                <div><dt>Date</dt><dd>{parsed?.invoice_date ?? "À vérifier"}</dd></div>
                <div><dt>HT</dt><dd>{parsed?.amount_ht ? `${parsed.amount_ht} €` : "À vérifier"}</dd></div>
                <div><dt>TVA</dt><dd>{parsed?.amount_vat ? `${parsed.amount_vat} €` : "À vérifier"}</dd></div>
                <div><dt>TTC</dt><dd>{parsed?.amount_ttc ? `${parsed.amount_ttc} €` : "À vérifier"}</dd></div>
                <div><dt>SIRET</dt><dd>{parsed?.siret ?? "Non détecté"}</dd></div>
                <div><dt>IBAN</dt><dd>{parsed?.iban ?? "Non détecté"}</dd></div>
              </dl>
              <p className={`check ${parsed?.amounts_consistent === true ? "ok" : parsed?.amounts_consistent === false ? "bad" : "neutral"}`}>
                {parsed?.amounts_consistent === true ? "✓ HT + TVA = TTC" : parsed?.amounts_consistent === false ? "⚠ HT + TVA ≠ TTC" : "Montants incomplets : contrôle à faire"}
              </p>
            </div>
            <div className="text-preview-inline"><pre>{selectedText}</pre></div>
          </div>
        </section>
      )}
    </main>
  );
}

export default App;
