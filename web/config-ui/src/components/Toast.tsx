import { useEffect } from 'react';
import { CheckCircle2, XCircle } from 'lucide-react';
import styles from './Toast.module.css';

export interface ToastMessage {
  id: number;
  tone: 'success' | 'error';
  text: string;
}

export function ToastStack({
  toasts,
  onDismiss,
}: {
  toasts: ToastMessage[];
  onDismiss: (id: number) => void;
}) {
  return (
    <div className={styles.stack}>
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} onDismiss={onDismiss} />
      ))}
    </div>
  );
}

function ToastItem({ toast, onDismiss }: { toast: ToastMessage; onDismiss: (id: number) => void }) {
  useEffect(() => {
    const timer = setTimeout(() => onDismiss(toast.id), 4000);
    return () => clearTimeout(timer);
  }, [toast.id, onDismiss]);

  const Icon = toast.tone === 'success' ? CheckCircle2 : XCircle;

  return (
    <div className={[styles.toast, toast.tone === 'success' ? styles.success : styles.error].join(' ')}>
      <Icon size={16} strokeWidth={2} />
      <span>{toast.text}</span>
    </div>
  );
}
