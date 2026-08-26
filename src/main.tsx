import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import ArchiveWorkspace from "./ArchiveWorkspace";
import CharlemagneMode from "./CharlemagneMode";
import CharlemagneTxtPreview from "./CharlemagneTxtPreview";
import ReadingOptimizer from "./ReadingOptimizer";
import StrictReviewGate from "./StrictReviewGate";
import "./styles.css";

function Root() {
  const [appRevision, setAppRevision] = useState(0);

  useEffect(() => {
    const refresh = () => setAppRevision((revision) => revision + 1);
    window.addEventListener("charlemagne-sync-updated", refresh);
    window.addEventListener("invoice-reading-updated", refresh);
    return () => {
      window.removeEventListener("charlemagne-sync-updated", refresh);
      window.removeEventListener("invoice-reading-updated", refresh);
    };
  }, []);

  return (
    <>
      <App key={appRevision} />
      <ArchiveWorkspace />
      <ReadingOptimizer />
      <StrictReviewGate />
      <CharlemagneMode />
      <CharlemagneTxtPreview />
    </>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
