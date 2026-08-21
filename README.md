# App Comptabiliter

Application Windows d'assistance a la comptabilite fournisseurs pour Charlemagne.

## Objectif V1

Le flux cible est :

`Dossier Windows ou depot PDF -> analyse de facture -> controle utilisateur -> classement automatique -> export/connecteur Charlemagne`

La secretaire conserve son fonctionnement actuel : elle depose les factures dans le dossier Windows partage. L'application peut aussi recevoir directement des PDF par glisser-deposer.

## Principes de securite

- Aucune ecriture n'est envoyee vers Charlemagne sans validation utilisateur.
- Une facture n'est deplacee vers son dossier final qu'apres validation.
- Les doublons, incoherences HT/TVA/TTC et fournisseurs inconnus doivent etre signales.
- L'integration Charlemagne passe par une interface documentee (API ou format d'import supporte), jamais par modification directe de sa base de donnees.

## Architecture cible

- Application desktop : Tauri 2
- Interface : React + TypeScript
- Acces fichiers Windows : Rust/Tauri
- Base locale : SQLite
- OCR/IA : module interchangeable ajoute apres stabilisation du flux fichiers
- Charlemagne : connecteur isole pour permettre API ou import de fichier

## Etapes

1. Socle Tauri + React + TypeScript
2. Depot de PDF dans l'application
3. Connexion et surveillance d'un dossier Windows
4. Base SQLite et historique des factures
5. Lecture PDF / OCR
6. Extraction structuree des informations facture
7. Regles fournisseurs et imputations comptables
8. Validation et classement automatique
9. Export/connecteur Charlemagne
10. Packaging Windows et tests
