import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";

type OptimizationResult = {
  inspected: number;
  promoted_99: number;
  deep_ocr: number;
  receipts_normalized: number;
  changed: number;
  errors: number;
};

type FocusedOptimizationResult = {
  inspected: number;
  processed: number;
  improved: number;
  errors: number;
};

type NativeNormalizationResult = {
  inspected: number;
  normalized: number;
  skipped: number;
  errors: number;
};

export type InvoiceReadingOptimizationSnapshot = {
  timestamp: string;
  native: NativeNormalizationResult;
  standard: OptimizationResult;
  focused: FocusedOptimizationResult;
  secondPass: OptimizationResult | null;
  totalInspected: number;
  totalChanged: number;
  totalErrors: number;
};

const SNAPSHOT_STORAGE_KEY = "app-comptabiliter:last-reading-optimization";

function publishSnapshot(snapshot: InvoiceReadingOptimizationSnapshot) {
  try {
    window.localStorage.setItem(SNAPSHOT_STORAGE_KEY, JSON.stringify(snapshot));
  } catch {
    // Les métriques de diagnostic ne doivent jamais bloquer le traitement.
  }
  window.dispatchEvent(
    new CustomEvent<InvoiceReadingOptimizationSnapshot>(
      "invoice-reading-optimization-completed",
      { detail: snapshot },
    ),
  );
}

export default function ReadingOptimizer() {
  const busy = useRef(false);

  useEffect(() => {
    let stopped = false;

    const optimize = async () => {
      if (stopped || busy.current) return;
      busy.current = true;
      try {
        // Les PDF avec une vraie couche texte sont d'abord normalisés avec les
        // formats réels observés dans le corpus fournisseur. Cela évite un OCR
        // inutile et conserve les chiffres exacts du PDF quand ils existent.
        const native = await invoke<NativeNormalizationResult>(
          "normalize_native_invoice_texts",
        );
        const result = await invoke<OptimizationResult>("optimize_invoice_readings");
        const focused = await invoke<FocusedOptimizationResult>(
          "optimize_focused_invoice_reading",
        );

        // Une passe focalisée peut révéler de nouveaux champs. On repasse
        // immédiatement dans le moteur de fusion/cohérence pour éviter
        // d'attendre le prochain cycle de 3 secondes.
        let secondPass: OptimizationResult | null = null;
        if (focused.improved > 0) {
          secondPass = await invoke<OptimizationResult>("optimize_invoice_readings");
        }

        if (stopped) return;

        const snapshot: InvoiceReadingOptimizationSnapshot = {
          timestamp: new Date().toISOString(),
          native,
          standard: result,
          focused,
          secondPass,
          totalInspected:
            native.inspected + result.inspected + focused.inspected +
            (secondPass?.inspected ?? 0),
          totalChanged:
            native.normalized + result.changed + focused.improved +
            (secondPass?.changed ?? 0),
          totalErrors:
            native.errors + result.errors + focused.errors +
            (secondPass?.errors ?? 0),
        };
        publishSnapshot(snapshot);

        if (snapshot.totalChanged > 0) {
          window.dispatchEvent(new Event("invoice-reading-updated"));
        }
      } catch {
        // Le flux principal garde ses propres erreurs visibles. Les optimisations
        // avancées restent opportunistes et ne doivent jamais bloquer l'interface.
      } finally {
        busy.current = false;
      }
    };

    void optimize();
    const interval = window.setInterval(() => void optimize(), 3000);
    return () => {
      stopped = true;
      window.clearInterval(interval);
    };
  }, []);

  return null;
}
