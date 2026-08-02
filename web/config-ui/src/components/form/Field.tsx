import type { ReactNode } from 'react';
import styles from './Field.module.css';

interface FieldProps {
  label: string;
  hint?: string;
  children: ReactNode;
  htmlFor?: string;
}

export function Field({ label, hint, children, htmlFor }: FieldProps) {
  return (
    <div className={styles.field}>
      <label className={styles.label} htmlFor={htmlFor}>
        {label}
      </label>
      {children}
      {hint && <p className={styles.hint}>{hint}</p>}
    </div>
  );
}

interface FieldGroupProps {
  title?: string;
  children: ReactNode;
  columns?: 1 | 2;
}

export function FieldGroup({ title, children, columns = 2 }: FieldGroupProps) {
  return (
    <section className={styles.group}>
      {title && <h3 className={styles.groupTitle}>{title}</h3>}
      <div className={columns === 2 ? styles.grid2 : styles.grid1}>{children}</div>
    </section>
  );
}
