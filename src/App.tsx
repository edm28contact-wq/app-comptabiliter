import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";

type Page = "accueil" | "factures" | "banque" | "journal" | "parametres";

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

type BankDocumentRecord = {
  path: string;
  file_name: string;
  status: string;
  extraction_status: string;
  text_length: number;
  duplicate_of: string | null;
  error: string | null;
};

type JournalEntryRow = {
  class_code: string;
  class_label: string;
  account: string;
  date: string;
  supplier: string;
  invoice_number: string;
  label: string;
  debit: string;
  credit: string;
  analytic_code: string | null;
  document_path: string | null;
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

type JournalTotals = {
  debit: number;
  credit: number;
  count: number;
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

const classLabels: Record<string, string> = {
  "1": "Capitaux",
  "2": "Immobilisations",
  "3": "Stocks et en-cours",
  "4": "Tiers",
  "5": "Financiers",
  "6": "Charges",
  "7": "Produits",
};

const isPdf = (path: string) => path.toLowerCase().endsWith(".pdf");
const amount = (value: string) => Number.parseFloat(value || "0") || 0;
const euro = (value: number) =>
  new Intl.NumberFormat("fr-FR", { style: "currency", currency: "EUR" }).format(value);

const extractionLabel = (status: string) => {
  if (status === "attente_stabilite") return "Copie en cours";
  if (status === "texte_extrait") return "Texte lu";
  if (status === "ocr_en_cours") return "OCR en cours";
  if (status === "ocr_requis") return "OCR à faire";
  if (status === "ocr_termine") return "OCR terminé";
  if (status === "doublon") return "Doublon";
  return "À analyser";
};

const invoiceStatusLabel = (status: string) => {
  if (status === "validee") return "Validée";
  if (status === "classee") return "Classée";
  if (status === "archive_erreur") return "Erreur archive";
  if (status === "archive_source_presente") return "Archive vérifiée";
  if (status === "doublon") return "Doublon";
  return "À vérifier";
};

const yearFromDate = (value: string) => {
  const parts = value.trim().split(/[./-]/);
  const rawYear = parts.at(-1) ?? "";
  if (/^\d{4}$/.test(rawYear)) return rawYear;
  if (/^\d{2}$/.test(rawYear)) return `20${rawYear}`;
  return null;
};

const totalsForEntries = (entries: JournalEntryRow[]): JournalTotals =>
  entries.reduce(
    (totals, entry) => ({
      debit: totals.debit + amount(entry.debit),
      credit: totals.credit + amount(entry.credit),
      count: totals.count + 1,
    }),
    { debit: 0, credit: 0, count: 0 },
  );

function App() {
  const [page, setPage] = useState<Page>("accueil");
  const [files, setFiles] = useState<InvoiceRecord[]>([]);
  const [bankFiles, setBankFiles] = useState<BankDocumentRecord[]>([]);
  const [journalEntries, setJournalEntries] = useState<JournalEntryRow[]>([]);
  const [dragging, setDragging] = useState(false);
  const [watchedFolder, setWatchedFolder] = useState<string | null>(null);
  const [bankFolder, setBankFolder] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [selectedText, setSelectedText] = useState<string | null>(null);
  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [bankPreview, setBankPreview] = useState<{ name: string; text: string } | null>(null);
  const [parsed, setParsed] = useState<ParsedInvoice>(emptyParsed);
  const [accounting, setAccounting] = useState<AccountingAssignment>(emptyAccounting);
  const [storage, setStorage] = useState<StorageAssignment>(emptyStorage);
  const [rememberRule, setRememberRule] = useState(true);
  const [rememberStorage, setRememberStorage] = useState(true);
  const [busyPath, setBusyPath] = useState<string | null>(null);
  const [journalClass, setJournalClass] = useState<string | null>(null);
  const [journalPrefix, setJournalPrefix] = useState<string | null>(null);
  const [journalAccount, setJournalAccount] = useState<string | null>(null);
  const [journalYear, setJournalYear] = useState(String(new Date().getFullYear()));
  const automaticOcrAttempted = useRef(new Set<string>());
  const invoiceScanBusy = useRef(false);
  const bankScanBusy = useRef(false);

  const refreshInvoices = async () => setFiles(await invoke<InvoiceRecord[]>("list_invoices"));
  const refreshBank = async () => setBankFiles(await invoke<BankDocumentRecord[]>("list_bank_documents"));
  const refreshJournal = async () => setJournalEntries(await invoke<JournalEntryRow[]>("list_journal_entries"));

  const refreshAll = async () => {
    const results = await Promise.allSettled([refreshInvoices(), refreshBank(), refreshJournal()]);
    const failure = results.find((result) => result.status === "rejected");
    if (failure?.status === "rejected") setMessage(String(failure.reason));
  };

  const registerPaths = async (paths: string[], source: string) => {
    for (const path of paths.filter(isPdf)) {
      await invoke("register_invoice", { path, source });
    }
    await refreshInvoices();
  };

  const scanInvoiceFolder = async (folder: string) => {
    if (invoiceScanBusy.current) return;
    invoiceScanBusy.current = true;
    try {
      await invoke("scan_pdf_folder", { path: folder });
      await refreshInvoices();
    } catch (error) {
      setMessage(`Source factures : ${String(error)}`);
    } finally {
      invoiceScanBusy.current = false;
    }
  };

  const scanBankFolder = async (folder: string) => {
    if (bankScanBusy.current) return;
    bankScanBusy.current = true;
    try {
      await invoke("scan_bank_folder", { path: folder });
      await refreshBank();
    } catch (error) {
      setMessage(`Source banque : ${String(error)}`);
    } finally {
      bankScanBusy.current = false;
    }
  };

  useEffect(() => {
    void (async () => {
      try {
        const [invoiceFolder, bankingFolder] = await Promise.all([
          invoke<string | null>("get_watched_folder"),
          invoke<string | null>("get_bank_watched_folder"),
        ]);
        setWatchedFolder(invoiceFolder);
        setBankFolder(bankingFolder);
        await refreshAll();
      } catch (error) {
        setMessage(String(error));
      }
    })();
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWindow()
      .onDragDropEvent((event) => {
        if (event.payload.type === "over") setDragging(true);
        if (event.payload.type === "leave") setDragging(false);
        if (event.payload.type === "drop") {
          setDragging(false);
          void registerPaths(event.payload.paths, "glisser-deposer");
        }
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    if (!watchedFolder) return;
    void scanInvoiceFolder(watchedFolder);
    const intervalId = window.setInterval(() => void scanInvoiceFolder(watchedFolder), 2500);
    return () => window.clearInterval(intervalId);
  }, [watchedFolder]);

  useEffect(() => {
    if (!bankFolder) return;
    void scanBankFolder(bankFolder);
    const intervalId = window.setInterval(() => void scanBankFolder(bankFolder), 2500);
    return () => window.clearInterval(intervalId);
  }, [bankFolder]);

  useEffect(() => {
    if (busyPath) return;
    const candidate = files.find(
      (file) =>
        file.extraction_status === "ocr_requis" &&
        file.status === "nouvelle" &&
        !automaticOcrAttempted.current.has(file.path),
    );
    if (!candidate) return;
    automaticOcrAttempted.current.add(candidate.path);
    void runOcr(candidate, true);
  }, [files, busyPath]);

  const chooseFiles = async () => {
    const selected = await open({
      multiple: true,
      directory: false,
      filters: [{ name: "Factures PDF", extensions: ["pdf"] }],
    });
    if (selected) await registerPaths(Array.isArray(selected) ? selected : [selected], "manuel");
  };

  const chooseInvoiceFolder = async () => {
    const selected = await open({ multiple: false, directory: true });
    if (!selected || Array.isArray(selected)) return;
    try {
      await invoke("set_watched_folder", { path: selected });
      setWatchedFolder(selected);
      setMessage(null);
    } catch (error) {
      setMessage(String(error));
    }
  };

  const chooseBankFolder = async () => {
    const selected = await open({ multiple: false, directory: true });
    if (!selected || Array.isArray(selected)) return;
    try {
      await invoke("set_bank_watched_folder", { path: selected });
      setBankFolder(selected);
      setMessage(null);
    } catch (error) {
      setMessage(String(error));
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
      setMessage(`Analyse : ${String(error)}`);
    } finally {
      setBusyPath(null);
    }
  };

  const runOcr = async (file: InvoiceRecord, automatic = false) => {
    setBusyPath(file.path);
    try {
      await invoke("run_invoice_ocr", { path: file.path });
      if (!automatic) setMessage(null);
    } catch (error) {
      if (!automatic) setMessage(`OCR : ${String(error)}`);
    } finally {
      await refreshInvoices();
      setBusyPath(null);
    }
  };

  const runBankOcr = async (file: BankDocumentRecord) => {
    setBusyPath(file.path);
    try {
      await invoke("run_bank_ocr", { path: file.path });
      setMessage(null);
    } catch (error) {
      setMessage(`OCR banque : ${String(error)}`);
    } finally {
      await refreshBank();
      setBusyPath(null);
    }
  };

  const retryArchive = async (file: InvoiceRecord) => {
    setBusyPath(file.path);
    try {
      await invoke<ArchiveResult>("archive_invoice", { path: file.path });
      setMessage(null);
      await refreshInvoices();
      await refreshJournal();
    } catch (error) {
      setMessage(`Archivage : ${String(error)}`);
      await refreshInvoices();
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
        invoke<AccountingAssignment | null>("get_supplier_accounting", {
          supplier: parsedData.supplier,
        }).then((value) => value ?? emptyAccounting),
        invoke<StorageAssignment | null>("get_supplier_storage", {
          supplier: parsedData.supplier,
        }).then((value) => value ?? emptyStorage),
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
    setPage("factures");
  };

  const inspectBank = async (file: BankDocumentRecord) => {
    const text = await invoke<string | null>("get_bank_document_text", { path: file.path });
    setBankPreview({
      name: file.file_name,
      text: text ?? "Aucun texte exploitable pour le moment.",
    });
  };

  const closeReview = () => {
    setSelectedText(null);
    setSelectedName(null);
    setSelectedPath(null);
    setAccounting(emptyAccounting);
    setStorage(emptyStorage);
  };

  const setField = (field: keyof ParsedInvoice, value: string) =>
    setParsed((current) => ({ ...current, [field]: value || null }));

  const setAccountingField = (field: keyof AccountingAssignment, value: string) =>
    setAccounting((current) => ({ ...current, [field]: value || null, source: "manuel" }));

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
            setMessage(`Archive vérifiée, mais la source reste présente : ${result.archive_path}`);
          } else {
            setMessage(null);
          }
        } catch (error) {
          setMessage(`Facture validée, archivage à reprendre : ${String(error)}`);
        }
      }
      await refreshInvoices();
      await refreshJournal();
      closeReview();
    } catch (error) {
      setMessage(`Validation : ${String(error)}`);
    } finally {
      setBusyPath(null);
    }
  };

  const pendingCount = files.filter((file) => file.status === "nouvelle").length;
  const errorCount = files.filter(
    (file) => file.status === "archive_erreur" || file.charlemagne_status === "incomplet",
  ).length;
  const readyCharlemagneCount = files.filter((file) => file.charlemagne_status === "pret").length;
  const bankPending = bankFiles.filter(
    (file) => file.status === "a_verifier" || file.status === "nouveau",
  ).length;

  const availableYears = useMemo(() => {
    const years = new Set<string>();
    years.add(String(new Date().getFullYear()));
    for (const entry of journalEntries) {
      const year = yearFromDate(entry.date);
      if (year) years.add(year);
    }
    return [...years].sort((left, right) => right.localeCompare(left));
  }, [journalEntries]);

  const yearEntries = useMemo(
    () =>
      journalYear === "all"
        ? journalEntries
        : journalEntries.filter((entry) => yearFromDate(entry.date) === journalYear),
    [journalEntries, journalYear],
  );

  const journalByClass = useMemo(() => {
    const map = new Map<string, JournalTotals>();
    for (const entry of yearEntries) {
      const current = map.get(entry.class_code) ?? { debit: 0, credit: 0, count: 0 };
      current.debit += amount(entry.debit);
      current.credit += amount(entry.credit);
      current.count += 1;
      map.set(entry.class_code, current);
    }
    return map;
  }, [yearEntries]);

  const classEntries = useMemo(
    () => (journalClass ? yearEntries.filter((entry) => entry.class_code === journalClass) : []),
    [journalClass, yearEntries],
  );

  const currentPrefix = journalPrefix ?? journalClass ?? "";
  const prefixEntries = useMemo(
    () =>
      journalClass
        ? classEntries.filter((entry) => entry.account.startsWith(currentPrefix))
        : [],
    [classEntries, currentPrefix, journalClass],
  );

  const childPrefixes = useMemo(() => {
    if (!journalClass || journalAccount || currentPrefix.length >= 4) return [];
    const nextLength = currentPrefix.length + 1;
    return [
      ...new Set(
        prefixEntries
          .map((entry) => entry.account)
          .filter((accountCode) => accountCode.length > currentPrefix.length)
          .map((accountCode) => accountCode.slice(0, nextLength)),
      ),
    ].sort();
  }, [currentPrefix, journalAccount, journalClass, prefixEntries]);

  const accountChoices = useMemo(
    () => [...new Set(prefixEntries.map((entry) => entry.account))].sort(),
    [prefixEntries],
  );

  const visibleJournalEntries = useMemo(
    () =>
      journalAccount
        ? classEntries.filter((entry) => entry.account === journalAccount)
        : prefixEntries,
    [classEntries, journalAccount, prefixEntries],
  );

  const supplierTotals = useMemo(() => {
    if (!journalAccount) return [];
    const grouped = new Map<string, JournalTotals>();
    for (const entry of visibleJournalEntries) {
      const supplier = entry.supplier || "Sans fournisseur";
      const current = grouped.get(supplier) ?? { debit: 0, credit: 0, count: 0 };
      current.debit += amount(entry.debit);
      current.credit += amount(entry.credit);
      current.count += 1;
      grouped.set(supplier, current);
    }
    return [...grouped.entries()].sort((left, right) => {
      const leftAmount = Math.max(left[1].debit, left[1].credit);
      const rightAmount = Math.max(right[1].debit, right[1].credit);
      return rightAmount - leftAmount;
    });
  }, [journalAccount, visibleJournalEntries]);

  const journalBreadcrumbs = useMemo(() => {
    if (!journalClass) return [];
    const prefixes: string[] = [];
    const terminal = journalPrefix ?? journalClass;
    for (let length = 2; length <= terminal.length; length += 1) {
      prefixes.push(terminal.slice(0, length));
    }
    return prefixes;
  }, [journalClass, journalPrefix]);

  const openJournalClass = (code: string) => {
    setJournalClass(code);
    setJournalPrefix(null);
    setJournalAccount(null);
  };

  const openJournalPrefix = (prefix: string) => {
    setJournalPrefix(prefix);
    setJournalAccount(null);
  };

  const journalBack = () => {
    if (journalAccount) {
      setJournalAccount(null);
      return;
    }
    if (journalPrefix && journalPrefix.length > 2) {
      const parent = journalPrefix.slice(0, -1);
      setJournalPrefix(parent === journalClass ? null : parent);
      return;
    }
    if (journalPrefix) {
      setJournalPrefix(null);
      return;
    }
    setJournalClass(null);
  };

  const resetJournal = () => {
    setJournalClass(null);
    setJournalPrefix(null);
    setJournalAccount(null);
  };

  const navigate = (target: Page) => {
    setPage(target);
    setBankPreview(null);
    if (target !== "journal") resetJournal();
  };

  const journalTitle = journalAccount
    ? `Compte ${journalAccount}`
    : journalPrefix
      ? `Comptes ${journalPrefix}…`
      : journalClass
        ? `Classe ${journalClass} — ${classLabels[journalClass] ?? "Comptes"}`
        : "Classes comptables";

  const showAccountChoices = Boolean(
    journalClass && !journalAccount && (currentPrefix.length >= 4 || childPrefixes.length === 0),
  );

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <strong>Compta Collège</strong>
          <span>Assistant Charlemagne</span>
        </div>
        <nav>
          <button className={page === "accueil" ? "active" : ""} onClick={() => navigate("accueil")}>Accueil</button>
          <button className={page === "factures" ? "active" : ""} onClick={() => navigate("factures")}>
            <span>Factures</span><b>{pendingCount}</b>
          </button>
          <button className={page === "banque" ? "active" : ""} onClick={() => navigate("banque")}>
            <span>Banque</span><b>{bankPending}</b>
          </button>
          <button className={page === "journal" ? "active" : ""} onClick={() => navigate("journal")}>Journal / Comptes</button>
          <button className={page === "parametres" ? "active" : ""} onClick={() => navigate("parametres")}>Paramètres</button>
        </nav>
        <div className="safety-note">
          Mode sécurisé<br />
          <span>Aucun envoi définitif vers Charlemagne.</span>
        </div>
      </aside>

      <div className="workspace">
        <header className="workspace-header">
          <div>
            <p className="eyebrow">Exercice comptable</p>
            <h1>
              {page === "accueil"
                ? "Tableau de bord"
                : page === "factures"
                  ? "Factures fournisseurs"
                  : page === "banque"
                    ? "Relevés bancaires"
                    : page === "journal"
                      ? "Journal / Comptes"
                      : "Paramètres"}
            </h1>
          </div>
          <span className="version-pill">TEST · V0.11</span>
        </header>

        {message && (
          <div className="global-message">
            <span>{message}</span>
            <button onClick={() => setMessage(null)}>Fermer</button>
          </div>
        )}

        {page === "accueil" && (
          <>
            <section className="summary-grid">
              <button onClick={() => navigate("factures")}>
                <span>Factures à vérifier</span><strong>{pendingCount}</strong><small>{files.length} document(s) connus</small>
              </button>
              <button onClick={() => navigate("banque")}>
                <span>Banque à traiter</span><strong>{bankPending}</strong><small>{bankFiles.length} relevé(s) connus</small>
              </button>
              <button onClick={() => navigate("journal")}>
                <span>Écritures journal</span><strong>{journalEntries.length}</strong><small>{readyCharlemagneCount} facture(s) préparées</small>
              </button>
              <button className={errorCount > 0 ? "has-error" : ""} onClick={() => navigate("factures")}>
                <span>Erreurs à traiter</span><strong>{errorCount}</strong><small>Archive ou préparation comptable</small>
              </button>
            </section>
            <section className="source-grid">
              <article className="source-card">
                <p className="eyebrow">Source 1</p><h2>Factures fournisseurs</h2>
                <p>{watchedFolder ?? "Dossier non connecté"}</p>
                <button onClick={() => navigate("factures")}>Ouvrir les factures</button>
              </article>
              <article className="source-card">
                <p className="eyebrow">Source 2</p><h2>Relevés bancaires</h2>
                <p>{bankFolder ?? "Dossier non connecté"}</p>
                <button onClick={() => navigate("banque")}>Ouvrir la banque</button>
              </article>
            </section>
            <section className="panel">
              <div className="panel-heading"><div><p className="eyebrow">Circuit sécurisé</p><h2>Traitement automatique</h2></div></div>
              <div className="pipeline"><span>PDF détecté</span><i>→</i><span>Lecture / OCR</span><i>→</i><span>Contrôle humain</span><i>→</i><span>Archivage vérifié</span><i>→</i><span>Journal</span></div>
            </section>
          </>
        )}

        {page === "factures" && (
          <>
            <section className="source-bar">
              <div><p className="eyebrow">Source automatique Factures</p><strong>{watchedFolder ?? "Aucun dossier connecté"}</strong></div>
              <button onClick={chooseInvoiceFolder}>{watchedFolder ? "Changer" : "Connecter"}</button>
            </section>
            <section className={`compact-dropzone ${dragging ? "is-dragging" : ""}`}>
              <div><strong>Ajouter des factures PDF</strong><span>Glisser-déposer ou sélection manuelle</span></div>
              <button onClick={chooseFiles}>Choisir des PDF</button>
            </section>
            <section className="panel">
              <div className="panel-heading"><div><h2>File Factures</h2><p>L'OCR démarre automatiquement si le PDF ne contient pas de texte exploitable.</p></div><span>{files.length}</span></div>
              {files.length === 0 ? (
                <div className="empty">Aucune facture.</div>
              ) : (
                <div className="document-list">
                  {files.map((file) => (
                    <article key={file.path} className="document-row">
                      <div className="document-main">
                        <strong>{file.file_name}</strong><small>{file.path}</small>
                        {file.archive_error && <em>{file.archive_error}</em>}
                        {file.charlemagne_error && <em>{file.charlemagne_error}</em>}
                        <div className="row-actions">
                          {(file.extraction_status === "texte_extrait" || file.extraction_status === "ocr_termine") && file.status !== "classee" && file.status !== "doublon" && (
                            <button onClick={() => inspectInvoice(file)}>Contrôler</button>
                          )}
                          {file.extraction_status === "ocr_requis" && (
                            <button disabled={busyPath === file.path} onClick={() => runOcr(file)}>{busyPath === file.path ? "OCR…" : "Réessayer OCR"}</button>
                          )}
                          {file.status === "nouvelle" && (
                            <button disabled={busyPath === file.path} onClick={() => reanalyze(file)}>Relire</button>
                          )}
                          {(file.status === "archive_erreur" || file.status === "archive_source_presente") && (
                            <button disabled={busyPath === file.path} onClick={() => retryArchive(file)}>Reprendre archivage</button>
                          )}
                        </div>
                      </div>
                      <div className="row-status">
                        <span>{extractionLabel(file.extraction_status)}</span>
                        <b className={`status-${file.status}`}>{invoiceStatusLabel(file.status)}</b>
                      </div>
                    </article>
                  ))}
                </div>
              )}
            </section>
          </>
        )}

        {page === "banque" && (
          <>
            <section className="source-bar bank">
              <div><p className="eyebrow">Source automatique Banque</p><strong>{bankFolder ?? "Aucun dossier connecté"}</strong></div>
              <button onClick={chooseBankFolder}>{bankFolder ? "Changer" : "Connecter"}</button>
            </section>
            <section className="panel">
              <div className="panel-heading">
                <div><h2>Relevés mensuels</h2><p>Cette source est indépendante du dossier Factures. Doublons et fichiers en cours de copie sont contrôlés séparément.</p></div>
                <span>{bankFiles.length}</span>
              </div>
              {bankFiles.length === 0 ? (
                <div className="empty">Aucun relevé bancaire détecté.</div>
              ) : (
                <div className="document-list">
                  {bankFiles.map((file) => (
                    <article key={file.path} className="document-row">
                      <div className="document-main">
                        <strong>{file.file_name}</strong><small>{file.path}</small>
                        {file.error && <em>{file.error}</em>}
                        {file.duplicate_of && <small>Doublon de : {file.duplicate_of}</small>}
                        <div className="row-actions">
                          {(file.extraction_status === "texte_extrait" || file.extraction_status === "ocr_termine") && (
                            <button onClick={() => inspectBank(file)}>Consulter</button>
                          )}
                          {file.extraction_status === "ocr_requis" && (
                            <button disabled={busyPath === file.path} onClick={() => runBankOcr(file)}>{busyPath === file.path ? "OCR…" : "OCR relevé"}</button>
                          )}
                        </div>
                      </div>
                      <div className="row-status"><span>{extractionLabel(file.extraction_status)}</span><b>{file.status === "doublon" ? "Doublon" : "À rapprocher"}</b></div>
                    </article>
                  ))}
                </div>
              )}
            </section>
            {bankPreview && (
              <section className="panel preview-panel">
                <div className="panel-heading"><h2>{bankPreview.name}</h2><button className="ghost" onClick={() => setBankPreview(null)}>Fermer</button></div>
                <pre>{bankPreview.text}</pre>
              </section>
            )}
          </>
        )}

        {page === "journal" && (
          <section className="panel journal-panel">
            <div className="panel-heading">
              <div>
                <h2>{journalTitle}</h2>
                <p>Navigation : classe → famille → sous-compte → compte → écritures. Les données viennent des factures validées et préparées localement.</p>
              </div>
              <div className="row-actions">
                <select value={journalYear} onChange={(event) => { setJournalYear(event.target.value); resetJournal(); }} aria-label="Exercice comptable">
                  {availableYears.map((year) => <option key={year} value={year}>Exercice {year}</option>)}
                  <option value="all">Tous les exercices</option>
                </select>
                {journalClass && <button className="ghost" onClick={journalBack}>Retour</button>}
                {journalClass && <button className="ghost" onClick={resetJournal}>Toutes les classes</button>}
              </div>
            </div>

            {journalClass && (
              <div className="account-strip">
                <button onClick={resetJournal}>Classes</button>
                <button className={!journalPrefix && !journalAccount ? "active" : ""} onClick={() => { setJournalPrefix(null); setJournalAccount(null); }}>Classe {journalClass}</button>
                {journalBreadcrumbs.map((prefix) => (
                  <button key={prefix} className={journalPrefix === prefix && !journalAccount ? "active" : ""} onClick={() => openJournalPrefix(prefix)}>{prefix}</button>
                ))}
                {journalAccount && <button className="active" onClick={() => setJournalAccount(journalAccount)}>{journalAccount}</button>}
              </div>
            )}

            {!journalClass ? (
              <div className="class-grid">
                {Object.entries(classLabels).map(([code, label]) => {
                  const totals = journalByClass.get(code) ?? { debit: 0, credit: 0, count: 0 };
                  return (
                    <button key={code} onClick={() => openJournalClass(code)}>
                      <div><strong>Classe {code}</strong><span>{label}</span></div>
                      <small>{totals.count} ligne(s)</small>
                      <b>Débit {euro(totals.debit)}</b><b>Crédit {euro(totals.credit)}</b>
                    </button>
                  );
                })}
              </div>
            ) : !journalAccount && childPrefixes.length > 0 ? (
              <div className="class-grid">
                {childPrefixes.map((prefix) => {
                  const entries = prefixEntries.filter((entry) => entry.account.startsWith(prefix));
                  const totals = totalsForEntries(entries);
                  return (
                    <button key={prefix} onClick={() => openJournalPrefix(prefix)}>
                      <div><strong>{prefix}</strong><span>Comptes commençant par {prefix}</span></div>
                      <small>{totals.count} ligne(s)</small>
                      <b>Débit {euro(totals.debit)}</b><b>Crédit {euro(totals.credit)}</b>
                    </button>
                  );
                })}
              </div>
            ) : showAccountChoices ? (
              <div className="class-grid">
                {accountChoices.map((accountCode) => {
                  const entries = classEntries.filter((entry) => entry.account === accountCode);
                  const totals = totalsForEntries(entries);
                  return (
                    <button key={accountCode} onClick={() => setJournalAccount(accountCode)}>
                      <div><strong>Compte {accountCode}</strong><span>{entries[0]?.label ?? "Écritures comptables"}</span></div>
                      <small>{totals.count} ligne(s)</small>
                      <b>Débit {euro(totals.debit)}</b><b>Crédit {euro(totals.credit)}</b>
                    </button>
                  );
                })}
                {accountChoices.length === 0 && <div className="empty">Aucun compte dans cette branche.</div>}
              </div>
            ) : (
              <>
                {supplierTotals.length > 0 && (
                  <div className="class-grid">
                    {supplierTotals.map(([supplier, totals]) => (
                      <button key={supplier} type="button">
                        <div><strong>{supplier}</strong><span>{totals.count} écriture(s) sur l'exercice</span></div>
                        <b>Débit {euro(totals.debit)}</b><b>Crédit {euro(totals.credit)}</b>
                      </button>
                    ))}
                  </div>
                )}
                <div className="journal-table-wrap">
                  <table>
                    <thead><tr><th>Date</th><th>Compte</th><th>Fournisseur</th><th>Facture</th><th>Libellé</th><th>Analytique</th><th>Débit</th><th>Crédit</th></tr></thead>
                    <tbody>
                      {visibleJournalEntries.map((entry, index) => (
                        <tr key={`${entry.account}-${entry.invoice_number}-${index}`}>
                          <td>{entry.date}</td><td><strong>{entry.account}</strong></td><td>{entry.supplier}</td><td>{entry.invoice_number}</td><td>{entry.label}</td><td>{entry.analytic_code ?? "—"}</td>
                          <td className="number">{amount(entry.debit) ? euro(amount(entry.debit)) : "—"}</td>
                          <td className="number">{amount(entry.credit) ? euro(amount(entry.credit)) : "—"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                  {visibleJournalEntries.length === 0 && <div className="empty">Aucune écriture dans ce compte pour cet exercice.</div>}
                </div>
              </>
            )}
          </section>
        )}

        {page === "parametres" && (
          <section className="settings-grid">
            <article className="settings-card"><p className="eyebrow">Source 1</p><h2>Dossier Factures</h2><p>{watchedFolder ?? "Non connecté"}</p><button onClick={chooseInvoiceFolder}>{watchedFolder ? "Modifier" : "Connecter"}</button></article>
            <article className="settings-card"><p className="eyebrow">Source 2</p><h2>Dossier Banque</h2><p>{bankFolder ?? "Non connecté"}</p><button onClick={chooseBankFolder}>{bankFolder ? "Modifier" : "Connecter"}</button></article>
            <article className="settings-card"><p className="eyebrow">Charlemagne</p><h2>Connecteur comptable</h2><p>Mode préparation locale. Aucun export propriétaire n'est généré tant que le format/API officiel n'est pas configuré.</p><span className="safe-badge">Sécurisé</span></article>
            <article className="settings-card"><p className="eyebrow">Fiabilité</p><h2>Protections actives</h2><p>SHA-256, détection de doublons, stabilité réseau, transactions SQLite, audit et archivage en deux phases.</p><span className="safe-badge">Actives</span></article>
          </section>
        )}

        {selectedText !== null && (
          <div className="modal-backdrop">
            <section className="review-modal">
              <div className="modal-heading"><div><p className="eyebrow">Contrôle humain</p><h2>{selectedName}</h2></div><button className="ghost" onClick={closeReview}>Fermer</button></div>
              <div className="review-grid">
                <div className="parsed-card">
                  <div className="confidence">Confiance extraction : <strong>{parsed.confidence}%</strong></div>
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
                    <div className="accounting-heading"><div><strong>Imputation comptable</strong><span>{accounting.source === "regle_fournisseur" ? `Règle connue · ${accounting.confidence}%` : "À contrôler"}</span></div></div>
                    <div className="form-grid">
                      <label>Compte fournisseur<input placeholder="401..." value={accounting.supplier_account ?? ""} onChange={(event) => setAccountingField("supplier_account", event.target.value)} /></label>
                      <label>Compte de charge<input placeholder="6..." value={accounting.expense_account ?? ""} onChange={(event) => setAccountingField("expense_account", event.target.value)} /></label>
                      <label>Compte TVA<input placeholder="445..." value={accounting.vat_account ?? ""} onChange={(event) => setAccountingField("vat_account", event.target.value)} /></label>
                      <label>Analytique<input value={accounting.analytic_code ?? ""} onChange={(event) => setAccountingField("analytic_code", event.target.value)} /></label>
                    </div>
                    <label className="checkbox"><input type="checkbox" checked={rememberRule} onChange={(event) => setRememberRule(event.target.checked)} />Mémoriser pour ce fournisseur</label>
                  </div>
                  <div className="accounting-card">
                    <div className="accounting-heading"><div><strong>Archivage</strong><span>{storage.source === "regle_fournisseur" ? "Dossier appris" : "Destination à contrôler"}</span></div></div>
                    <div className="archive-choice"><span>{storage.archive_folder ?? "Aucun dossier sélectionné"}</span><button onClick={chooseArchiveFolder}>Choisir</button></div>
                    <label className="checkbox"><input type="checkbox" checked={rememberStorage} onChange={(event) => setRememberStorage(event.target.checked)} />Mémoriser le dossier</label>
                  </div>
                  <p className={`check ${parsed.amounts_consistent === true ? "ok" : parsed.amounts_consistent === false ? "bad" : "neutral"}`}>
                    {parsed.amounts_consistent === true ? "HT + TVA = TTC" : parsed.amounts_consistent === false ? "HT + TVA ne correspond pas au TTC" : "Montants à compléter"}
                  </p>
                  <button className="validate" disabled={busyPath === selectedPath} onClick={validate}>{busyPath === selectedPath ? "VALIDATION…" : storage.archive_folder ? "VALIDER ET CLASSER" : "VALIDER"}</button>
                </div>
                <div className="text-preview"><pre>{selectedText}</pre></div>
              </div>
            </section>
          </div>
        )}
      </div>
    </main>
  );
}

export default App;
