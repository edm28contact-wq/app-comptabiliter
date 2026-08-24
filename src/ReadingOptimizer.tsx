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

export default function ReadingOptimizer() {
  const busy = useRef(false);

  useEffect(() => {
    let stopped = false;

    const optimize = async () => {
      if (stopped || busy.current) return;
      busy.current = true;
      try {
        const result = await invoke<OptimizationResult>("optimize_invoice_readings");
        if (!stopped && result.changed > 0) {
          window.dispatchEvent(new Event("invoice-reading-updated"));
        }
      } catch {
        // Le flux principal garde ses propres erreurs visibles. L'optimiseur
        // est opportuniste et ne doit jamais bloquer l'interface.
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
