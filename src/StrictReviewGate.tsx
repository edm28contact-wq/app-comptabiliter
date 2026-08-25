import { useEffect } from "react";

const STRICT_THRESHOLD = 99;

const NON_INVOICE_MARKERS: Array<[RegExp, string]> = [
  [/il\s+ne\s+s['’]?agit\s+pas\s+d['’]?une\s+facture\s+tva/i, "Document explicitement non TVA"],
  [/\bcertificat\s+de\s+garantie\b/i, "Certificat de garantie"],
  [/\battestation\s+(?:de\s+)?gravage\b/i, "Attestation"],
  [/\b(?:fiche|convention)\s+de\s+stage\b/i, "Document de stage"],
  [/\bbon\s+de\s+commande\b/i, "Bon de commande"],
  [/\baccord\s+pour\s+commande\b/i, "Bon de commande"],
];

const normalizeAmount = (value: string) => {
  const normalized = value
    .trim()
    .replace(/[€$£]/g, "")
    .replace(/\b(?:EUR|CAD|USD|GBP)\b/gi, "")
    .replace(/[\u00a0\u202f\s]/g, "");
  if (!normalized) return Number.NaN;
  const comma = normalized.lastIndexOf(",");
  const dot = normalized.lastIndexOf(".");
  let numeric = normalized;
  if (comma >= 0 && dot >= 0) {
    numeric = comma > dot
      ? normalized.replace(/\./g, "").replace(",", ".")
      : normalized.replace(/,/g, "");
  } else if (comma >= 0) {
    numeric = normalized.replace(",", ".");
  }
  return Number.parseFloat(numeric);
};

const fieldValue = (card: Element, labelName: string) => {
  const labels = Array.from(card.querySelectorAll(".form-grid > label"));
  const label = labels.find((candidate) => {
    const text = Array.from(candidate.childNodes)
      .filter((node) => node.nodeType === Node.TEXT_NODE)
      .map((node) => node.textContent ?? "")
      .join(" ")
      .trim()
      .toLocaleLowerCase("fr-FR");
    return text === labelName.toLocaleLowerCase("fr-FR");
  });
  return (label?.querySelector("input") as HTMLInputElement | null)?.value.trim() ?? "";
};

const validDate = (value: string) => {
  const match = value.trim().match(/^(\d{1,2})[./-](\d{1,2})[./-](\d{2}|\d{4})$/);
  if (!match) return false;
  const day = Number(match[1]);
  const month = Number(match[2]);
  const year = Number(match[3].length === 2 ? `20${match[3]}` : match[3]);
  return day >= 1 && day <= 31 && month >= 1 && month <= 12 && year >= 1900 && year <= 2100;
};

const readRawConfidence = (confidence: Element) => {
  const value = confidence.querySelector("strong")?.textContent?.match(/\d+/)?.[0];
  return value ? Number.parseInt(value, 10) : 0;
};

const ensureBadge = (confidence: Element) => {
  let badge = confidence.querySelector<HTMLElement>("[data-strict-review-badge]");
  if (!badge) {
    badge = document.createElement("span");
    badge.dataset.strictReviewBadge = "true";
    confidence.appendChild(badge);
  }
  return badge;
};

const detectNonInvoice = (modal: Element) => {
  const text = modal.textContent ?? "";
  return NON_INVOICE_MARKERS.find(([pattern]) => pattern.test(text))?.[1] ?? null;
};

const setValidationBlocked = (modal: Element, blocked: boolean) => {
  const buttons = Array.from(modal.querySelectorAll<HTMLButtonElement>("button"));
  for (const button of buttons) {
    const label = (button.textContent ?? "").trim().toLocaleLowerCase("fr-FR");
    if (!label.startsWith("valider")) continue;

    if (blocked) {
      if (!button.dataset.strictReviewBlocked) {
        button.dataset.strictReviewPreviousDisabled = button.disabled ? "true" : "false";
      }
      button.dataset.strictReviewBlocked = "true";
      button.disabled = true;
      button.title = "Ce document a été reconnu comme non-facture. Il ne peut pas créer d'écriture fournisseur.";
      continue;
    }

    if (button.dataset.strictReviewBlocked === "true") {
      button.disabled = button.dataset.strictReviewPreviousDisabled === "true";
      delete button.dataset.strictReviewBlocked;
      delete button.dataset.strictReviewPreviousDisabled;
      if (button.title.startsWith("Ce document a été reconnu")) button.removeAttribute("title");
    }
  }
};

const evaluateReview = (modal: Element) => {
  const card = modal.querySelector(".parsed-card");
  const confidence = modal.querySelector(".confidence");
  if (!card || !confidence) return;

  const nonInvoiceReason = detectNonInvoice(modal);
  const rawConfidence = readRawConfidence(confidence);
  const supplier = fieldValue(card, "Fournisseur");
  const invoiceNumber = fieldValue(card, "N° facture");
  const date = fieldValue(card, "Date");
  const ht = normalizeAmount(fieldValue(card, "HT"));
  const vat = normalizeAmount(fieldValue(card, "TVA"));
  const ttc = normalizeAmount(fieldValue(card, "TTC"));
  const complete =
    supplier.length >= 2 &&
    invoiceNumber.length >= 3 &&
    validDate(date) &&
    Number.isFinite(ht) &&
    Number.isFinite(vat) &&
    Number.isFinite(ttc);
  const arithmeticOk = complete && Math.abs(ht + vat - ttc) <= 0.02;
  const manuallyEdited = modal.getAttribute("data-manual-edited") === "true";
  const strictPass =
    !nonInvoiceReason &&
    rawConfidence >= STRICT_THRESHOLD &&
    complete &&
    arithmeticOk &&
    !manuallyEdited;

  modal.classList.toggle("strict-review-ok", strictPass);
  modal.classList.toggle("strict-review-manual", !strictPass);
  modal.classList.toggle("strict-review-non-invoice", Boolean(nonInvoiceReason));
  confidence.classList.toggle("strict-confidence-ok", strictPass);
  confidence.classList.toggle("strict-confidence-manual", !strictPass);
  setValidationBlocked(modal, Boolean(nonInvoiceReason));

  const badge = ensureBadge(confidence);
  if (nonInvoiceReason) {
    badge.textContent = `DOCUMENT NON FACTURE — ${nonInvoiceReason.toLocaleUpperCase("fr-FR")}`;
    badge.setAttribute("data-state", "non-invoice");
    return;
  }
  if (strictPass) {
    badge.textContent = "LECTURE FIABLE ≥ 99%";
    badge.setAttribute("data-state", "ok");
    return;
  }

  badge.textContent = manuallyEdited ? "SAISIE MANUELLE EN COURS" : "SAISIE MANUELLE REQUISE";
  badge.setAttribute("data-state", "manual");
};

const evaluateAll = () => {
  document.querySelectorAll(".review-modal").forEach(evaluateReview);
};

export default function StrictReviewGate() {
  useEffect(() => {
    const observer = new MutationObserver(evaluateAll);
    observer.observe(document.body, { childList: true, subtree: true, characterData: true });

    const onInput = (event: Event) => {
      const target = event.target;
      if (!(target instanceof HTMLInputElement)) return;
      const modal = target.closest(".review-modal");
      if (!modal) return;
      modal.setAttribute("data-manual-edited", "true");
      evaluateReview(modal);
    };

    document.addEventListener("input", onInput, true);
    evaluateAll();
    return () => {
      observer.disconnect();
      document.removeEventListener("input", onInput, true);
    };
  }, []);

  return (
    <style>{`
      .confidence [data-strict-review-badge] {
        display: block;
        margin-top: 7px;
        font-size: 12px;
        font-weight: 900;
        letter-spacing: .03em;
      }
      .confidence [data-strict-review-badge][data-state="ok"] {
        color: #17653a;
      }
      .confidence [data-strict-review-badge][data-state="manual"] {
        color: #a51f1f;
      }
      .confidence [data-strict-review-badge][data-state="non-invoice"] {
        color: #7a3e00;
      }
      .strict-confidence-ok {
        border: 1px solid #a8d8b8;
        background: #e8f7ee !important;
        color: #17653a !important;
      }
      .strict-confidence-manual {
        border: 2px solid #d94343;
        background: #fff0f0 !important;
        color: #8f2020 !important;
      }
      .strict-review-non-invoice .confidence {
        border-color: #c97812 !important;
        background: #fff7e8 !important;
        color: #7a3e00 !important;
      }
      .strict-review-manual .parsed-card {
        box-shadow: inset 5px 0 0 #d94343;
        background: #fffafa;
      }
      .strict-review-non-invoice .parsed-card {
        box-shadow: inset 5px 0 0 #c97812;
        background: #fffaf2;
      }
      .strict-review-manual .parsed-card > .form-grid input {
        border-color: #e6b3b3;
      }
      .strict-review-manual .parsed-card > .form-grid input:focus {
        border-color: #c62f2f;
        outline-color: #ffdada;
      }
      .strict-review-ok .parsed-card {
        box-shadow: inset 5px 0 0 #3f9b61;
      }
    `}</style>
  );
}
