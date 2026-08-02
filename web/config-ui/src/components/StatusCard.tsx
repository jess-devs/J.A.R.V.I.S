import styles from './StatusCard.module.css';

interface StatusCardProps {
  label: string;
  ok: boolean | null;
  okText: string;
  badText: string;
  unknownText?: string;
  detail: string;
}

/** Anillo de progreso: completo en verde si `ok`, casi vacío en rojo si no. */
function Ring({ ok }: { ok: boolean | null }) {
  const radius = 15;
  const circumference = 2 * Math.PI * radius;
  const fraction = ok === null ? 0.08 : ok ? 1 : 0.12;
  const color = ok === null ? 'var(--text-tertiary)' : ok ? 'var(--positive)' : 'var(--danger)';
  return (
    <svg width="36" height="36" viewBox="0 0 36 36" aria-hidden="true">
      <circle cx="18" cy="18" r={radius} fill="none" stroke="var(--surface-secondary)" strokeWidth="3" />
      <circle
        cx="18"
        cy="18"
        r={radius}
        fill="none"
        stroke={color}
        strokeWidth="3"
        strokeLinecap="round"
        strokeDasharray={circumference}
        strokeDashoffset={circumference * (1 - fraction)}
        transform="rotate(-90 18 18)"
      />
    </svg>
  );
}

export function StatusCard({ label, ok, okText, badText, unknownText, detail }: StatusCardProps) {
  const value = ok === null ? (unknownText ?? '—') : ok ? okText : badText;
  return (
    <div className={styles.card}>
      <Ring ok={ok} />
      <div className={styles.body}>
        <span className={styles.label}>{label}</span>
        <span className={[styles.value, ok === false ? styles.valueBad : ''].join(' ')}>{value}</span>
        <p className={styles.detail}>{detail}</p>
      </div>
    </div>
  );
}
