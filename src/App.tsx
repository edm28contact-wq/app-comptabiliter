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

type AccountingAssignment = {
  supplier_account: string | null;
  expense_account: string | null;
  vat_account: string | null;
  analytic_code: string | null;
  confidence: number;
  source: string;
  use_count: number;
};

type StorageAssignment = {
  archive_folder: string | null;
  confidence: number;
  source: string;
  use_count: number;
};

const emptyParsed: ParsedInvoice = {
  supplier: null,
  invoice_number: null,
  invoice_date: null,
  amount_ht: null,
  amount_vat: null,
  amount_ttc: null,
  siret: null,
  iban: null,
  amounts_consistent: null,
  confidence: 0,
};

const emptyAccounting: AccountingAssignment = {
  supplier_account: null,
  expense_account: null,
  vat_account: null,
  analytic_code: null,
  confidence: 0,
  source: "manuel",
  use_count: 0,
};

const emptyStorage: StorageAssignment = {
  archive_folder: null,
  confidence: 0,
  source: "manuel",
  use_count: 0,
};

const isPdf = (path: string) => path.toLowerCase().endsWith(".pdf");

const extractionLabel = (status: string) => {
  if (status === "texte_extrait") return "Texte lu";
  if (status === "ocr_requis") return "OCR requis";
  if (status === "ocr_termine") return "OCR terminé";
  return "À analyser";
};

function App() {
  const [files, setFiles] = useState<InvoiceRecord[]>([]);
  const [dragging, setDragging] = useState(false);
  const [watchedFolder, setWatchedFolder] = useState<string | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [selectedText, setSelectedText] = useState<string | null>(null);
  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [parsed, setParsed] = useState<ParsedInvoice>(emptyParsed);
  const [accounting, setAccounting] = useState<AccountingAssignment>(emptyAccounting);
  const [storage, setStorage] = useState<StorageAssignment>(emptyStorage);
  const [rememberRule, setRememberRule] = useState(true);
  const [rememberStorage, setRememberStorage] = useState(true);
  const [busyPath, setBusyPath] = useState<string | null>(null);

  const refreshInvoices = async () => {
    setFiles(await invoke<InvoiceRecord[]>("list_invoices"));
  };

  const registerPaths = async (paths: string[], source: string) => {
    await Promise.all(paths.filter(isPdf).map((path) => invoke("register_invoice", { path, source })));
    await refreshInvoices();
  };

  const scanFolder = async (folder: string) => {
    try {
      await invoke("scan_pdf_folder", { path: folder });
      await refreshInvoices();
      setFolderError(null);
    } catch (error) {
      setFolderError(String(error));
    }
  };

  useEffect(() => {
    void (async () => {
      try {
        setWatchedFolder(await invoke<string | null>("get_watched_folder"));
        await refreshInvoices();
      } catch (error) {
        setFolderError(String(error));
      }
    })();
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
    const intervalId = window.setInterval(() => void scanFolder(watchedFolder), 2000);
    return () => window.clearInterval(intervalId);
  }, [watchedFolder]);

  const chooseFiles = async () => {
    const selected = await open({ multiple: true, directory: false, filters: [{ name: "Factures PDF", extensions: ["pdf"] }] });
    if (selected) await registerPaths(Array.isArray(selected) ? selected : [selected], "manuel");
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

  const chooseArchiveFolder = async () => {
    const selected = await open({ multiple: false, directory: true });
    if (!selected || Array.isArray(selected)) return;
    setStorage({ archive_folder: selected, confidence: 0, source: "manuel", use_count: 0 });
  };

  const reanalyze = async (file: InvoiceRecord) => {
    setBusyPath(file.path);
    try {
      await invoke("analyze_invoice", { path: file.path });
      await refreshInvoices();
    } finally {
      setBusyPath(null);
    }
  };

  const runOcr = async (file: InvoiceRecord) => {
    setBusyPath(file.path);
    try {
      await invoke("run_invoice_ocr", { path: file.path });
      await refreshInvoices();
    } catch (error) {
      setFolderError(`OCR : ${String(error)}`);
    } finally {
      setBusyPath(null);
    }
  };

  const inspectInvoice = async (file: InvoiceRecord) => {
    const [text, data] = await Promise.all([
      invoke<string | null>("get_invoice_text", { path: file.path }),
      invoke<ParsedInvoice | null>("get_invoice_parsed", { path: file.path }),
    ]);

    const parsedData = data ?? emptyParsed;
    let accountingRule = emptyAccounting;
    let storageRule = emptyStorage;
    if (parsedData.supplier) {
      [accountingRule, storageRule] = await Promise.all([
        invoke<AccountingAssignment | null>("get_supplier_accounting", { supplier: parsedData.supplier }).then((value) => value ?? emptyAccounting),
        invoke<StorageAssignment | null>("get_supplier_storage", { supplier: parsedData.supplier }).then((value) => value ?? emptyStorage),
      ]);
    }

    setSelectedPath(file.path);
    setSelectedName(file.file_name);
    setSelectedText(text ?? "Aucun texte extrait.");
    setParsed(parsedData);
    setAccounting(accountingRule);
    setStorage(storageRule);
    setRememberRule(true);
    setRememberStorage(true);
  };

  const setField = (field: keyof ParsedInvoice, value: string) => {
    setParsed((current) => ({ ...current, [field]: value || null }));
  };

  const setAccountingField = (field: keyof AccountingAssignment, value: string) => {
    setAccounting((current) => ({ ...current, [field]: value || null, source: "manuel" }));
  };

  const validate = async () => {
    if (!selectedPath) return;
    await invoke("validate_invoice", {
      path: selectedPath,
      data: parsed,
      accounting,
      storage,
      rememberRule,
      rememberStorage,
    });
    await refreshInvoices();
    setSelectedText(null);
    setSelectedName(null);
    setSelectedPath(null);
    setAccounting(emptyAccounting);
    setStorage(emptyStorage);
  };

  const pendingCount = files.filter((file) => file.status === "nouvelle").length;
  const validatedCount = files.filter((file) => file.status === "validee").length;
  const ocrCount = files.filter((file) => file.extraction_status === "ocr_requis").length;

  return (
    <main className="shell">
      <header className="topbar">
        <div><p className="eyebrow">Assistant Charlemagne</p><h1>Factures fournisseurs</h1></div>
        <span className="status">V0.8 · Classement appris</span>
      </header>

      <section className="stats">
        <article><strong>{files.length}</strong><span>Factures enregistrées</span></article>
        <article><strong>{pendingCount}</strong><span>À vérifier</span></article>
        <article><strong>{ocrCount}</strong><span>OCR requis</span></article>
        <article><strong>{validatedCount}</strong><span>Validées</span></article>
      </section>

      <section className="folder-card">
        <div><p className="eyebrow">Source automatique</p><h2>Dossier Windows surveillé</h2><p className="folder-path">{watchedFolder ?? "Aucun dossier connecté."}</p>{folderError && <p className="error">{folderError}</p>}</div>
        <button type="button" onClick={chooseFolder}>{watchedFolder ? "Changer de dossier" : "Connecter un dossier"}</button>
      </section>

      <section className={`dropzone ${dragging ? "is-dragging" : ""}`}>
        <div className="drop-icon">PDF</div><h2>Déposez vos factures ici</h2><p>Glissez des PDF depuis Windows ou sélectionnez-les manuellement.</p><button type="button" onClick={chooseFiles}>Ajouter des factures</button>
      </section>

      <section className="queue">
        <div className="section-heading"><h2>File de traitement</h2><span>{files.length} document{files.length > 1 ? "s" : ""}</span></div>
        {files.length === 0 ? <div className="empty">Aucune facture enregistrée.</div> : (
          <ul>{files.map((file) => (
            <li key={file.path}>
              <div className="file-info"><strong>{file.file_name}</strong><small>{file.path} · source : {file.source}</small>
                <div className="file-actions">
                  {(file.extraction_status === "texte_extrait" || file.extraction_status === "ocr_termine") && <button type="button" className="secondary" onClick={() => inspectInvoice(file)}>Contrôler</button>}
                  {file.extraction_status === "ocr_requis" && <button type="button" className="secondary" disabled={busyPath === file.path} onClick={() => runOcr(file)}>{busyPath === file.path ? "OCR…" : "Lancer OCR"}</button>}
                  <button type="button" className="secondary" disabled={busyPath === file.path} onClick={() => reanalyze(file)}>Réanalyser</button>
                </div>
              </div>
              <div className="badges"><span className={`extraction ${file.extraction_status}`}>{extractionLabel(file.extraction_status)}{file.text_length > 0 ? ` · ${file.text_length} car.` : ""}</span><span className="pending">{file.status}</span></div>
            </li>
          ))}</ul>
        )}
      </section>

      {selectedText !== null && (
        <section className="review-panel">
          <div className="section-heading"><h2>Contrôle · {selectedName}</h2><button type="button" className="secondary" onClick={() => setSelectedText(null)}>Fermer</button></div>
          <div className="review-grid">
            <div className="parsed-card">
              <div className="confidence">Confiance extraction initiale : <strong>{parsed.confidence}%</strong></div>
              <div className="form-grid">
                <label>Fournisseur<input value={parsed.supplier ?? ""} onChange={(e) => setField("supplier", e.target.value)} /></label>
                <label>N° facture<input value={parsed.invoice_number ?? ""} onChange={(e) => setField("invoice_number", e.target.value)} /></label>
                <label>Date<input value={parsed.invoice_date ?? ""} onChange={(e) => setField("invoice_date", e.target.value)} /></label>
                <label>HT<input value={parsed.amount_ht ?? ""} onChange={(e) => setField("amount_ht", e.target.value)} /></label>
                <label>TVA<input value={parsed.amount_vat ?? ""} onChange={(e) => setField("amount_vat", e.target.value)} /></label>
                <label>TTC<input value={parsed.amount_ttc ?? ""} onChange={(e) => setField("amount_ttc", e.target.value)} /></label>
                <label>SIRET<input value={parsed.siret ?? ""} onChange={(e) => setField("siret", e.target.value)} /></label>
                <label>IBAN<input value={parsed.iban ?? ""} onChange={(e) => setField("iban", e.target.value)} /></label>
              </div>

              <div className="accounting-card">
                <div className="accounting-heading"><div><strong>Imputation comptable</strong><span>{accounting.source === "regle_fournisseur" ? `Règle connue · confiance ${accounting.confidence}%` : "À renseigner"}</span></div></div>
                <div className="form-grid accounting-grid">
                  <label>Compte fournisseur<input placeholder="401..." value={accounting.supplier_account ?? ""} onChange={(e) => setAccountingField("supplier_account", e.target.value)} /></label>
                  <label>Compte de charge<input placeholder="6..." value={accounting.expense_account ?? ""} onChange={(e) => setAccountingField("expense_account", e.target.value)} /></label>
                  <label>Compte TVA<input placeholder="445..." value={accounting.vat_account ?? ""} onChange={(e) => setAccountingField("vat_account", e.target.value)} /></label>
                  <label>Analytique<input placeholder="Code analytique" value={accounting.analytic_code ?? ""} onChange={(e) => setAccountingField("analytic_code", e.target.value)} /></label>
                </div>
                <label className="remember-rule"><input type="checkbox" checked={rememberRule} onChange={(e) => setRememberRule(e.target.checked)} /> Mémoriser cette imputation pour ce fournisseur</label>
              </div>

              <div className="accounting-card">
                <div className="accounting-heading"><div><strong>Classement Windows</strong><span>{storage.source === "regle_fournisseur" ? `Dossier connu · confiance ${storage.confidence}%` : "Choisir un dossier d'archive existant"}</span></div></div>
                <div className="archive-row">
                  <div className="archive-path">{storage.archive_folder ?? "Aucun dossier d'archive sélectionné."}</div>
                  <button type="button" className="secondary" onClick={chooseArchiveFolder}>Choisir</button>
                </div>
                <label className="remember-rule"><input type="checkbox" checked={rememberStorage} onChange={(e) => setRememberStorage(e.target.checked)} /> Mémoriser ce dossier pour ce fournisseur</label>
              </div>

              <p className={`check ${parsed.amounts_consistent === true ? "ok" : parsed.amounts_consistent === false ? "bad" : "neutral"}`}>{parsed.amounts_consistent === true ? "✓ HT + TVA = TTC" : parsed.amounts_consistent === false ? "⚠ HT + TVA ≠ TTC" : "Montants incomplets : contrôle à faire"}</p>
              <button type="button" className="validate" onClick={validate}>VALIDER LA FACTURE</button>
            </div>
            <div className="text-preview-inline"><pre>{selectedText}</pre></div>
          </div>
        </section>
      )}
    </main>
  );
}

export default App;
