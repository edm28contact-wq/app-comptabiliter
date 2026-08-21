import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";

const isPdf = (path: string) => path.toLowerCase().endsWith(".pdf");

function App() {
  const [files, setFiles] = useState<string[]>([]);
  const [dragging, setDragging] = useState(false);

  const addFiles = (paths: string[]) => {
    setFiles((current) => Array.from(new Set([...current, ...paths.filter(isPdf)])));
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

  const chooseFiles = async () => {
    const selected = await open({
      multiple: true,
      directory: false,
      filters: [{ name: "Factures PDF", extensions: ["pdf"] }]
    });

    if (!selected) return;
    addFiles(Array.isArray(selected) ? selected : [selected]);
  };

  return (
    <main className="shell">
      <header className="topbar">
        <div>
          <p className="eyebrow">Assistant Charlemagne</p>
          <h1>Factures fournisseurs</h1>
        </div>
        <span className="status">V0.1 · Socle</span>
      </header>

      <section className="stats">
        <article><strong>{files.length}</strong><span>Factures chargées</span></article>
        <article><strong>0</strong><span>À vérifier</span></article>
        <article><strong>0</strong><span>Validées</span></article>
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
