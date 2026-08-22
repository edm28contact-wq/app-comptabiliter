import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import CharlemagneMode from "./CharlemagneMode";
import "./styles.css";

function Root() {
  const [appRevision, setAppRevision] = useState(0);

  useEffect(() => {
    const refresh = () => setAppRevision((revision) => revision + 1);
    window.addEventListener("charlemagne-sync-updated", refresh);
    return () => window.removeEventListener("charlemagne-sync-updated", refresh);
  }, []);

  return (
    <>
      <App key={appRevision} />
      <CharlemagneMode />
    </>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
