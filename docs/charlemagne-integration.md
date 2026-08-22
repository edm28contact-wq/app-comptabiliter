# Intégration Charlemagne Comptabilité

## État actuel

L'application prépare une écriture comptable intermédiaire à partir de données validées. Cette écriture n'est pas encore sérialisée dans un format Charlemagne spécifique et n'est jamais envoyée automatiquement.

## Données déjà disponibles

Pour chaque facture validée, l'application peut fournir :

- fournisseur ;
- numéro de facture ;
- date ;
- HT ;
- TVA ;
- TTC ;
- compte fournisseur ;
- compte de charge ;
- compte TVA ;
- code analytique ;
- chemin du justificatif ;
- lignes débit/crédit équilibrées ;
- état de préparation et erreurs éventuelles.

## Informations à obtenir d'Aplim / Charlemagne

Demander l'une des solutions officiellement supportées suivantes, par ordre de préférence :

1. documentation API/SDK ou accès partenaire ;
2. spécification du fichier d'import des écritures depuis un autre logiciel ;
3. exemple réel de fichier d'écritures accepté par Charlemagne ;
4. documentation de la passerelle GED externe pour associer le justificatif ;
5. règles de nommage/identification des journaux, exercices et établissements ;
6. format attendu des comptes auxiliaires, analytiques et références de pièce ;
7. mécanisme de retour permettant de savoir si un import a été accepté ou rejeté.

## Échantillons utiles à demander

Idéalement obtenir :

- un fichier exporté par Azopio, Yooz, Zeendoc ou une autre solution officiellement compatible, anonymisé si nécessaire ;
- le même jeu d'écritures visible dans Charlemagne après import ;
- un exemple avec TVA ;
- un exemple sans TVA ;
- un exemple avec analytique ;
- un exemple comportant un justificatif GED lié.

## Règles de sécurité de l'adaptateur

L'adaptateur final devra respecter ces contraintes :

- aucune écriture ne part sans validation humaine initiale ;
- aucun compte n'est inventé ;
- débit = crédit avant export ;
- l'exercice et le journal doivent être explicitement configurés ;
- le fichier source et le justificatif archivé restent traçables ;
- chaque tentative d'export doit être journalisée ;
- un rejet Charlemagne ne doit jamais supprimer les données locales ;
- un export réussi doit conserver l'identifiant ou la preuve de l'import si Charlemagne en fournit une.

## Architecture cible

```text
Facture validée
      |
      v
PreparedCharlemagneEntry
      |
      v
CharlemagneConnector
      |
      +-- ApiConnector        si API officielle disponible
      |
      +-- ImportFileConnector si format d'import officiel disponible
      |
      +-- DisabledConnector   tant qu'aucun format officiel n'est configuré
```

L'application doit rester indépendante du format final. Seul le connecteur transforme l'écriture normalisée en représentation Charlemagne.
