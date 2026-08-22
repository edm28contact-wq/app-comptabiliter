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
  archive_path: string | null;
  archive_error: string | null;
  charlemagne_status: string;
  charlemagne_error: string | null;
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

type ArchiveResult = {
  archive_path: string;
  content_hash: string;
  source_deleted: boolean;
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

const invoiceStatusLabel = (status: string) => {
  if (status === "validee") return "Validée";
  if (status === "classee") return "Classée";
  if (status === "archive_erreur") return "Erreur archive";
  if (status === "archive_source_presente") return "Archivée · source présente";
  return "À vérifier";
};

const charlemagneLabel = (status: string) => {
  if (status === "pret") return "Prête Charlemagne";
  if (status === "incomplet") return "Charlemagne incomplet";
  return "Charlemagne à préparer";
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
    } catch (error) {
      setFolderError(`Analyse : ${String(error)}`);
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

  const retryArchive = async (file: InvoiceRecord) => {
    setBusyPath(file.path);
    try {
      await invoke<ArchiveResult>("archive_invoice", { path: file.path });
      setFolderError(null);
      await refreshInvoices();
    } catch (error) {
      setFolderError(`Archivage : ${String(error)}`);
      await refreshInvoices();
    } finally {
      setBusyPath(null);
    }
  };

  const prepareCharlemagne = async (file: InvoiceRecord) => {
    setBusyPath(file.path);
    try {
      await invoke("prepare_charlemagne_invoice", { path: file.path });
      setFolderError(null);
    } catch (error) {
      setFolderError(`Charlemagne : ${String(error)}`);
    } finally {
      await refreshInvoices();
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

  const closeReview = () => {
    setSelectedText(null);
    setSelectedName(null);
    setSelectedPath(null);
    setAccounting(emptyAccounting);
    setStorage(emptyStorage);
  };

  const validate = async () => {
    if (!selectedPath) return;
    const invoicePath = selectedPath;
    setBusyPath(invoicePath);
    try {
      await invoke("validate_invoice", {
        path: invoicePath,
        data: parsed,
        accounting,
        storage,
        rememberRule,
        rememberStorage,
      });

      if (storage.archive_folder) {
        try {
          const result = await invoke<ArchiveResult>("archive_invoice", { path: invoicePath });
          if (!result.source_deleted) {
            setFolderError(`Archive vérifiée : ${result.archive_path}. Le fichier source n'a pas pu être supprimé.`);
          } else {
            setFolderError(null);
          }
        } catch (error) {
          setFolderError(`Facture validée, mais archivage impossible : ${String(error)}`);
        }
      }

      await refreshInvoices();
      closeReview();
    } catch (error) {
      setFolderError(`Validation : ${String(error)}`);
    } finally {
      setBusyPath(null);
    }
  };

  const pendingCount = files.filter((file) => file.status === "nouvelle").length;
  const validatedCount = files.filter((file) => ["validee", "classee", "archive_source_presente"].includes(file.status)).length;
  const readyCharlemagneCount = files.filter((file) => file.charlemagne_status === "pret").length;
  const ocrCount = files.filter((file) => file.extraction_status === "ocr_requis").length;

  return (
    <main className="shell">
      <header className="topbar">
        <div><p className="eyebrow">Assistant Charlemagne</p><h1>Factures fournisseurs</h1></div>
        <span className="status">V0.10 · Préparation Charlemagne</span>
      </header>

      <section className="stats">
        <article><strong>{files.length}</strong><span>Factures enregistrées</span></article>
        <article><strong>{pendingCount}</strong><span>À vérifier</span></article>
        <article><strong>{ocrCount}</strong><span>OCR requis</span></article>
        <article><strong>{readyCharlemagneCount}/{validatedCount}</strong><span>Prêtes Charlemagne</span></article>
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
              <div className="file-info">
                <strong>{file.file_name}</strong>
                <small>{file.path} · source : {file.source}</small>
                {file.archive_path && <small className="archive-success">Archive : {file.archive_path}</small>}
                {file.archive_error && <small className="archive-failure">Erreur archive : {file.archive_error}</small>}
                {file.charlemagne_error && <small className="charlemagne-failure">Charlemagne : {file.charlemagne_error}</small>}
                <div className="file-actions">
                  {(file.extraction_status === "texte_extrait" || file.extraction_status === "ocr_termine") && file.status !== "classee" && <button type="button" className="secondary" onClick={() => inspectInvoice(file)}>Contrôler</button>}
                  {file.extraction_status === "ocr_requis" && file.status === "nouvelle" && <button type="button" className="secondary" disabled={busyPath === file.path} onClick={() => runOcr(file)}>{busyPath === file.path ? "OCR…" : "Lancer OCR"}</button>}
                  {file.status === "nouvelle" && <button type="button" className="secondary" disabled={busyPath === file.path} onClick={() => reanalyze(file)}>Réanalyser</button>}
                  {file.status === "archive_erreur" && <button type="button" className="secondary" disabled={busyPath === file.path} onClick={() => retryArchive(file)}>{busyPath === file.path ? "Archivage…" : "Réessayer archivage"}</button>}
                  {file.status !== "nouvelle" && file.charlemagne_status !== "pret" && <button type="button" className="secondary" disabled={busyPath === file.path} onClick={() => prepareCharlemagne(file)}>{busyPath === file.path ? "Préparation…" : "Préparer Charlemagne"}</button>}
                </div>
              </div>
              <div className="badges">
                <span className={`charlemagne ${file.charlemagne_status}`}>{charlemagneLabel(file.charlemagne_status)}</span>
                <span className={`extraction ${file.extraction_status}`}>{extractionLabel(file.extraction_status)}{file.text_length > 0 ? ` · ${file.text_length} car.` : ""}</span>
                <span className={`pending status-${file.status}`}>{invoiceStatusLabel(file.status)}</span>
              </div>
            </li>
          ))}</ul>
        )}
      </section>

      {selectedText !== null && (
        <section className="review-panel">
          <div className="section-heading"><h2>Contrôle · {selectedName}</h2><button type="button" className="secondary" onClick={closeReview}>Fermer</button></div>
          <div className="review-grid">
            <div className="parsed-card">
              <div className="confidence">Confiance extraction initiale : <strong>{parsed.confidence}%</strong></div>
              <div className="form-grid">
                <label>Fournisseur<input value={parsed.supplier ?? ""} onChange={(event) => setField("supplier", event.target.value)} /></label>
                <label>N° facture<input value={parsed.invoice_number ?? ""} onChange={(event) => setField("invoice_number", event.target.value)} /></label>
                <label>Date<input value={parsed.invoice_date ?? ""} onChange={(event) => setField("invoice_date", event.target.value)} /></label>
                <label>HT<input value={parsed.amount_ht ?? ""} onChange={(event) => setField("amount_ht", event.target.value)} /></label>
                <label>TVA<input value={parsed.amount_vat ?? ""} onChange={(event) => setField("amount_vat", event.target.value)} /></label>
                <label>TTC<input value={parsed.amount_ttc ?? ""} onChange={(event) => setField("amount_ttc", event.target.value)} /></label>
                <label>SIRET<input value={parsed.siret ?? ""} onChange={(event) => setField("siret", event.target.value)} /></label>
                <label>IBAN<input value={parsed.iban ?? ""} onChange={(event) => setField("iban", event.target.value)} /></label>
              </div>

              <div className="accounting-card">
                <div className="accounting-heading"><div><strong>Imputation comptable</strong><span>{accounting.source === "regle_fournisseur" ? `Règle connue · confiance ${accounting.confidence}%` : "À renseigner"}</span></div></div>
                <div className="form-grid accounting-grid">
                  <label>Compte fournisseur<input placeholder="401..." value={accounting.supplier_account ?? ""} onChange={(event) => setAccountingField("supplier_account", event.target.value)} /></label>
                  <label>Compte de charge<input placeholder="6..." value={accounting.expense_account ?? ""} onChange={(event) => setAccountingField("expense_account", event.target.value)} /></label>
                  <label>Compte TVA<input placeholder="445..." value={accounting.vat_account ?? ""} onChange={(event) => setAccountingField("vat_account", event.target.value)} /></label>
                  <label>Analytique<input placeholder="Code analytique" value={accounting.analytic_code ?? ""} onChange={(event) => setAccountingField("analytic_code", event.target.value)} /></label>
                </div>
                <label className="remember-rule"><input type="checkbox" checked={rememberRule} onChange={(event) => setRememberRule(event.target.checked)} /> Mémoriser cette imputation pour ce fournisseur</label>
              </div>

              <div className="accounting-card">
                <div className="accounting-heading"><div><strong>Classement Windows</strong><span>{storage.source === "regle_fournisseur" ? `Dossier connu · confiance ${storage.confidence}%` : "Choisir un dossier d'archive existant"}</span></div></div>
                <div className="archive-row">
                  <div className="archive-path">{storage.archive_folder ?? "Aucun dossier d'archive sélectionné. La facture sera validée sans être déplacée."}</div>
                  <button type="button" className="secondary" onClick={chooseArchiveFolder}>Choisir</button>
                </div>
                <label className="remember-rule"><input type="checkbox" checked={rememberStorage} onChange={(event) => setRememberStorage(event.target.checked)} /> Mémoriser ce dossier pour ce fournisseur</label>
              </div>

              <p className={`check ${parsed.amounts_consistent === true ? "ok" : parsed.amounts_consistent === false ? "bad" : "neutral"}`}>{parsed.amounts_consistent === true ? "✓ HT + TVA = TTC" : parsed.amounts_consistent === false ? "⚠ HT + TVA ≠ TTC" : "Montants incomplets : contrôle à faire"}</p>
              <button type="button" className="validate" disabled={busyPath === selectedPath} onClick={validate}>{busyPath === selectedPath ? "VALIDATION…" : storage.archive_folder ? "VALIDER ET CLASSER" : "VALIDER LA FACTURE"}</button>
            </div>
            <div className="text-preview-inline"><pre>{selectedText}</pre></div>
          </div>
        </section>
      )}
    </main>
  );
}

export default App;
