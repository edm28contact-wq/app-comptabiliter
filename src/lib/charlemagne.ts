export type DebitCredit = "D" | "C";

export interface CharlemagneEntry {
  date: string;
  journal: string;
  account: string;
  accountLabel: string;
  entryLabel: string;
  amountCents: number;
  direction: DebitCredit;
}

const HEADER = [
  "Date",
  "Journal",
  "Compte",
  "Libel Compte",
  "",
  "LibelEcriture",
  "Montant",
  "D/C",
  "",
  "",
] as const;

const CP1252_EXTRA = new Map<number, number>([
  [0x20ac, 0x80], [0x201a, 0x82], [0x0192, 0x83], [0x201e, 0x84],
  [0x2026, 0x85], [0x2020, 0x86], [0x2021, 0x87], [0x02c6, 0x88],
  [0x2030, 0x89], [0x0160, 0x8a], [0x2039, 0x8b], [0x0152, 0x8c],
  [0x017d, 0x8e], [0x2018, 0x91], [0x2019, 0x92], [0x201c, 0x93],
  [0x201d, 0x94], [0x2022, 0x95], [0x2013, 0x96], [0x2014, 0x97],
  [0x02dc, 0x98], [0x2122, 0x99], [0x0161, 0x9a], [0x203a, 0x9b],
  [0x0153, 0x9c], [0x017e, 0x9e], [0x0178, 0x9f],
]);

function sanitize(value: string): string {
  if (/\t|\r|\n/.test(value)) {
    throw new Error("Les libellés Charlemagne ne peuvent pas contenir de tabulation ou de saut de ligne.");
  }
  return value;
}

function formatDate(value: string): string {
  const compact = value.replaceAll("-", "");
  if (!/^\d{8}$/.test(compact)) {
    throw new Error(`Date Charlemagne invalide: ${value}`);
  }
  return compact;
}

function formatAmount(cents: number): string {
  if (!Number.isSafeInteger(cents) || cents < 0) {
    throw new Error(`Montant en centimes invalide: ${cents}`);
  }
  const euros = Math.floor(cents / 100);
  const decimals = String(cents % 100).padStart(2, "0");
  return decimals === "00" ? String(euros) : `${euros},${decimals}`;
}

function row(entry: CharlemagneEntry): string[] {
  return [
    formatDate(entry.date),
    sanitize(entry.journal),
    sanitize(entry.account),
    sanitize(entry.accountLabel),
    "",
    sanitize(entry.entryLabel),
    formatAmount(entry.amountCents),
    entry.direction,
    "",
    "",
  ];
}

export function serializeCharlemagne(entries: readonly CharlemagneEntry[]): string {
  return [HEADER, ...entries.map(row)].map((columns) => columns.join("\t")).join("\r\n") + "\r\n";
}

export function encodeWindows1252(value: string): Uint8Array {
  const bytes: number[] = [];

  for (const character of value) {
    const codePoint = character.codePointAt(0)!;
    if (codePoint <= 0x7f || (codePoint >= 0xa0 && codePoint <= 0xff)) {
      bytes.push(codePoint);
      continue;
    }

    const mapped = CP1252_EXTRA.get(codePoint);
    if (mapped === undefined) {
      throw new Error(`Caractère non compatible Windows-1252: ${character}`);
    }
    bytes.push(mapped);
  }

  return Uint8Array.from(bytes);
}

export function buildCharlemagneFile(entries: readonly CharlemagneEntry[]): Uint8Array {
  return encodeWindows1252(serializeCharlemagne(entries));
}
