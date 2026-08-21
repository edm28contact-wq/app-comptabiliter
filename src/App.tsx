import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";

const isPdf = (path: string) => path.toLowerCase().endsWith(".pdf");

function App() {
  const [files, setFiles] = useState<string[]>([]);
  const [dragging, setDragging] = useState(false);
  const [watchedFolder, setWatchedFolder] = useState<string | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);

  const addFiles = (paths: string[]) => {
    setFiles((current) => Array.from(new Set([...current, ...paths.filter(isPdf)])));
  };

  const scanFolder = async (folder: string) => {
    try {
      const pdfs = await invoke<string[]>("scan_pdf_folder", { path: folder });
      addFiles(pdfs);
      setFolderError(null);
    } catch (error) {
      setFolderError(String(error));
    }
  };

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    getCurrentWindow()
      .onDragDropEvent((event) => {
        if (event.payload.type === "over") setDragging(true);
        if (event.payload.type === "leave") setDragging(false);
        if (event.payload.type === "drop") {
          setDragging(false);
          addFiles(event.payload.paths);
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
    addFiles(Array.isArray(selected) ? selected : [selected]);
  };

  const chooseFolder = async () => {
    const selected = await open({
      multiple: false,
      directory: true
    });

    if (!selected || Array.isArray(selected)) return;
    setWatchedFolder(selected);
  };

  return (
    <main className="shell">
      <header className="topbar">
        <div>
          <p className="eyebrow">Assistant Charlemagne</p>
          <h1>Factures fournisseurs</h1>
        </div>
        <span className="status">V0.2 · Dossier Windows</span>
      </header>

      <section className="stats">
        <article><strong>{files.length}</strong><span>Factures chargées</span></article>
        <article><strong>0</strong><span>À vérifier</span></article>
        <article><strong>0</strong><span>Validées</span></article>
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
          <div className="empty">Aucune facture chargée.</div>
        ) : (
          <ul>
            {files.map((path) => {
              const name = path.split(/[\\/]/).pop() ?? path;
              return (
                <li key={path}>
                  <div>
                    <strong>{name}</strong>
                    <small>{path}</small>
                  </div>
                  <span className="pending">En attente</span>
                </li>
              );
            })}
          </ul>
        )}
      </section>
    </main>
  );
}

export default App;
