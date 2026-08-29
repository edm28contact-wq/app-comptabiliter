import { describe, expect, it } from "vitest";
import { buildCharlemagneFile, serializeCharlemagne } from "./charlemagne";

const entry = {
  date: "2025-11-30",
  journal: "OD",
  account: "64510000",
  accountLabel: "Sécurité sociale URSSAF",
  entryLabel: "OD Salaires Nov. 2025: Sécurité sociale URSSAF",
  amountCents: 539722,
  direction: "D" as const,
};

describe("Charlemagne export", () => {
  it("produit exactement 10 colonnes tabulées avec CRLF", () => {
    const text = serializeCharlemagne([entry]);
    const lines = text.split("\r\n").filter(Boolean);

    expect(lines).toHaveLength(2);
    expect(lines[0].split("\t")).toHaveLength(10);
    expect(lines[1].split("\t")).toHaveLength(10);
    expect(lines[1]).toBe(
      "20251130\tOD\t64510000\tSécurité sociale URSSAF\t\tOD Salaires Nov. 2025: Sécurité sociale URSSAF\t5397,22\tD\t\t",
    );
  });

  it("encode les accents en Windows-1252", () => {
    const bytes = buildCharlemagneFile([entry]);
    const text = serializeCharlemagne([entry]);
    const accentIndex = text.indexOf("é");

    expect(bytes[accentIndex]).toBe(0xe9);
  });

  it("refuse les caractères non représentables en Windows-1252", () => {
    expect(() =>
      buildCharlemagneFile([{ ...entry, accountLabel: "Test 😀" }]),
    ).toThrow("Windows-1252");
  });
});
