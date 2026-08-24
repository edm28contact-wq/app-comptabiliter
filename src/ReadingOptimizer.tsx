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

export default function ReadingOptimizer() {
  const busy = useRef(false);

  useEffect(() => {
    let stopped = false;

    const optimize = async () => {
      if (stopped || busy.current) return;
      busy.current = true;
      try {
        const result = await invoke<OptimizationResult>("optimize_invoice_readings");
        const focused = await invoke<FocusedOptimizationResult>(
          "optimize_focused_invoice_reading",
        );

        // Une passe focalisée peut révéler de nouveaux champs. On repasse
        // immédiatement dans le moteur de fusion/cohérence pour éviter
        // d'attendre le prochain cycle de 3 secondes.
        let secondPassChanged = 0;
        if (focused.improved > 0) {
          const second = await invoke<OptimizationResult>("optimize_invoice_readings");
          secondPassChanged = second.changed;
        }

        if (
          !stopped &&
          (result.changed > 0 || focused.improved > 0 || secondPassChanged > 0)
        ) {
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