export default function App() {
  return (
    <main className="shell">
      <section className="panel">
        <p className="eyebrow">App Comptabiliter</p>
        <h1>Connecteur Charlemagne</h1>
        <p>
          Le format d'import observé est maintenant verrouillé dans un module dédié et testable.
        </p>
        <dl>
          <div><dt>Séparateur</dt><dd>Tabulation</dd></div>
          <div><dt>Encodage</dt><dd>Windows-1252</dd></div>
          <div><dt>Fin de ligne</dt><dd>CRLF</dd></div>
          <div><dt>Colonnes</dt><dd>10 fixes</dd></div>
          <div><dt>Montant</dt><dd>Virgule décimale</dd></div>
          <div><dt>Sens</dt><dd>D / C</dd></div>
        </dl>
      </section>
    </main>
  );
}
