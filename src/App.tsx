import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";

type InvoiceRecord = {
  path: string;
  file_name: string;
  source: string;
  status: string;
};

const isPdf = (path: string) => path.toLowerCase().endsWith(".pdf");

function App() {
  const [files, setFiles] = useState<InvoiceRecord[]>([]);
  const [dragging, setDragging] = useState(false);
  const [watchedFolder, setWatchedFolder] = useState<string | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);

  const refreshInvoices = async () => {
    const records = await invoke<InvoiceRecord[]>("list_invoices");
    setFiles(records);
  };

  const registerPaths = async (paths: string[], source: string) => {
    const pdfs = paths.filter(isPdf);
    await Promise.all(
      pdfs.map((path) => invoke("register_invoice", { path, source }))
    );
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

    void scanFolder(watchedFolder);
    const intervalId = window.setInterval(() => {
      void scanFolder(watchedFolder);
    }, 2000);

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
    const selected = await open({
      multiple: false,
      directory: true
    });

    if (!selected || Array.isArray(selected)) return;

    try {
      await invoke("set_watched_folder", { path: selected });
      setWatchedFolder(selected);
      setFolderError(null);
    } catch (error) {
      setFolderError(String(error));
    }
  };

  const pendingCount = files.filter((file) => file.status === "nouvelle").length;
  const validatedCount = files.filter((file) => file.status === "validee").length;

  return (
    <main className="shell">
      <header className="topbar">
        <div>
          <p className="eyebrow">Assistant Charlemagne</p>
          <h1>Factures fournisseurs</h1>
        </div>
        <span className="status">V0.3 · Persistance locale</span>
      </header>

      <section className="stats">
        <article><strong>{files.length}</strong><span>Factures enregistrées</span></article>
        <article><strong>{pendingCount}</strong><span>À vérifier</span></article>
        <article><strong>{validatedCount}</strong><span>Validées</span></article>
      </section>

      <section className="folder-card">
        <div>
          <p className="eyebrow">Source automatique</p>
          <h2>Dossier Windows surveillé</h2>
          <p className="folder-path">
            {watchedFolder ?? "Aucun dossier connecté pour le moment."}
          </p>
          {folderError && <p className="error">{folderError}</p>}
        </div>
        <button type="button" onClick={chooseFolder}>
          {watchedFolder ? "Changer de dossier" : "Connecter un dossier"}
        </button>
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

        {files.length === 0 ? (
          <div className="empty">Aucune facture enregistrée.</div>
        ) : (
          <ul>
            {files.map((file) => (
              <li key={file.path}>
                <div>
                  <strong>{file.file_name}</strong>
                  <small>{file.path} · source : {file.source}</small>
                </div>
                <span className="pending">{file.status}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </main>
  );
}

export default App;
