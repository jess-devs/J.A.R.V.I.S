import { RotateCcw, Save, TriangleAlert } from "lucide-react";
import styles from "./SaveBar.module.css";

interface SaveBarProps {
  dirty: boolean;
  saving: boolean;
  needsRestart: boolean;
  onSave: () => void;
  onDiscard: () => void;
}

export function SaveBar({
  dirty,
  saving,
  needsRestart,
  onSave,
  onDiscard,
}: SaveBarProps) {
  return (
    <div className={styles.bar}>
      <div className={styles.status}>
        {needsRestart && (
          <span className={styles.restartNote}>
            <TriangleAlert size={14} strokeWidth={2} />
            Reiniciá Jarvis para aplicar los últimos cambios guardados.
          </span>
        )}
        {!needsRestart && dirty && (
          <span className={styles.dirtyNote}>Tenés cambios sin guardar.</span>
        )}
      </div>
      <div className={styles.actions}>
        <button
          type="button"
          className={styles.secondary}
          onClick={onDiscard}
          disabled={!dirty || saving}
        >
          <RotateCcw size={14} strokeWidth={2} />
          Descartar
        </button>
        <button
          type="button"
          className={styles.primary}
          onClick={onSave}
          disabled={!dirty || saving}
        >
          <Save size={14} strokeWidth={2} />
          {saving ? "Guardando…" : "Guardar cambios"}
        </button>
      </div>
    </div>
  );
}
