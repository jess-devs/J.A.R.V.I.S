import styles from './VuMeter.module.css';

const MIN_DBFS = -60;
const MAX_DBFS = 0;

function levelPercent(dbfs: number | null): number {
  if (dbfs === null) return 0;
  const clamped = Math.min(MAX_DBFS, Math.max(MIN_DBFS, dbfs));
  return ((clamped - MIN_DBFS) / (MAX_DBFS - MIN_DBFS)) * 100;
}

export function VuMeter({ dbfs }: { dbfs: number | null }) {
  const percent = levelPercent(dbfs);
  const tone = percent > 85 ? styles.hot : percent > 15 ? styles.active : styles.quiet;

  return (
    <div
      className={styles.track}
      role="meter"
      aria-label="Nivel de micrófono"
      aria-valuemin={MIN_DBFS}
      aria-valuemax={MAX_DBFS}
      aria-valuenow={dbfs ?? MIN_DBFS}
    >
      <div className={[styles.fill, tone].join(' ')} style={{ width: `${percent}%` }} />
    </div>
  );
}
