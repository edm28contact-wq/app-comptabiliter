# Plan de test Windows — App Comptabiliter

## Objectif

Valider le comportement réel de l'application sur un poste Windows et sur un partage réseau avant toute utilisation quotidienne.

## Préparation

- Installer l'application sur un poste Windows 10 ou 11.
- Utiliser un dossier local de test puis un partage réseau SMB réel.
- Préparer un jeu de PDF contenant :
  - PDF texte natif ;
  - PDF scanné ;
  - facture avec TVA ;
  - facture sans TVA ;
  - facture avec montant supérieur à 1 000 € ;
  - deux copies strictement identiques d'une même facture ;
  - fichier PDF volontairement incomplet pendant quelques secondes ;
  - fournisseur connu et fournisseur inconnu.

## Tests ingestion

1. Glisser-déposer un PDF texte.
   - Le document apparaît une seule fois dans la file.
   - Le texte est extrait.
2. Importer le même PDF depuis le sélecteur.
   - Le doublon doit être détecté lorsque le dédoublonnage par hash sera actif.
3. Déposer un PDF scanné.
   - Le statut doit devenir `OCR requis`.
   - L'OCR manuel doit produire du texte ou une erreur explicite.
4. Copier lentement un PDF dans le dossier surveillé.
   - L'application ne doit pas analyser un fichier tant qu'il est encore en écriture lorsque la détection de stabilité sera active.
5. Déconnecter temporairement le partage réseau.
   - L'application doit conserver sa file SQLite.
   - Aucune facture déjà connue ne doit être perdue.

## Tests extraction et validation

1. Vérifier fournisseur, numéro, date, HT, TVA et TTC.
2. Corriger volontairement un champ puis valider.
3. Vérifier que la valeur validée reste distincte de la valeur initialement détectée.
4. Vérifier `HT + TVA = TTC`.
5. Tester une facture sans TVA.
6. Tester une facture avec montant français `1.248,72`.

## Tests apprentissage fournisseur

1. Renseigner compte fournisseur, charge, TVA et analytique.
2. Cocher la mémorisation et valider.
3. Charger une seconde facture du même fournisseur.
4. Vérifier que les comptes sont proposés automatiquement.
5. Modifier une proposition et valider pour confirmer que l'apprentissage est mis à jour seulement après validation humaine.

## Tests archivage

1. Sélectionner un dossier d'archive local.
2. Valider et classer.
3. Vérifier le nom du fichier généré.
4. Vérifier que la copie existe et correspond au hash source.
5. Vérifier que la source n'est supprimée qu'après copie vérifiée.
6. Tester une collision de nom : le second fichier doit recevoir `_2`, puis `_3`, etc.
7. Tester un dossier réseau inaccessible : statut `archive_erreur`, source conservée.
8. Réactiver le partage et utiliser `Réessayer archivage`.

## Tests préparation Charlemagne

1. Facture complète : statut `Prête Charlemagne`.
2. Compte fournisseur absent : statut `Charlemagne incomplet`.
3. Compte de charge absent : statut `Charlemagne incomplet`.
4. TVA non nulle et compte TVA absent : statut `Charlemagne incomplet`.
5. Montants déséquilibrés : préparation refusée.
6. Après archivage, vérifier que le chemin du justificatif préparé pointe vers l'archive définitive.
7. Aucun envoi réel vers Charlemagne ne doit être possible tant que l'adaptateur officiel n'est pas configuré.

## Tests Banque

1. Relevé test : solde initial 1 000,00 €.
2. Débit EDF 1 248,72 €.
3. Crédit virement 500,00 €.
4. Frais bancaires 15,00 €.
5. Solde final attendu : 236,28 €.
6. Le contrôle doit être équilibré.
7. Modifier le solde final à 250,00 € : le contrôle doit échouer.

## Critères avant mise en production

- CI Windows verte.
- Aucun déplacement/suppression de source non vérifié.
- Tests d'archivage local et réseau réussis.
- OCR vérifié sur plusieurs scans réels.
- Jeu de factures fournisseurs représentatif testé.
- Sauvegarde SQLite testée.
- Format d'import Charlemagne validé avec Aplim avant activation de tout export.
