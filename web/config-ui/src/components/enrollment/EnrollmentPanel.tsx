import { useEffect, useState } from 'react';
import { AlertTriangle, CheckCircle2, Mic, X } from 'lucide-react';
import { api } from '../../api/client';
import type { SpeakerVerificationStatus } from '../../api/types';
import { useVoiceEnrollment } from '../../hooks/useVoiceEnrollment';
import styles from './EnrollmentPanel.module.css';

const ENROLL_SECONDS = 5;

function formatEnrolledAt(iso: string | null): string {
  if (!iso) return '';
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

/**
 * Grabación guiada de la voz de referencia para `speaker_verification`
 * (ver `docs/planning/verificacion-hablante.md`, A.4). Vive dentro de
 * `OnboardingSection` ("Micrófono"), debajo del panel de calibración de
 * dispositivo — reusa `cfg.input_device_index` de ahí, sin selector propio.
 * No abre el micrófono hasta que el usuario aprieta "Grabar".
 */
export function EnrollmentPanel({
  deviceIndex,
  deviceLabel,
}: {
  deviceIndex: number | null;
  deviceLabel: string;
}) {
  const [active, setActive] = useState(false);
  const [status, setStatus] = useState<SpeakerVerificationStatus | null>(null);

  const refreshStatus = () => {
    api
      .statusSpeakerVerification()
      .then(setStatus)
      .catch(() => setStatus(null));
  };

  useEffect(refreshStatus, [active]);

  if (active) {
    return (
      <ActiveEnrollment
        deviceIndex={deviceIndex}
        onDone={() => {
          setActive(false);
        }}
      />
    );
  }

  return (
    <div className={styles.panel}>
      <p className={styles.hint}>
        {status?.enrolled
          ? `Ya tenés una voz enrolada (grabada el ${formatEnrolledAt(status.enrolled_at)}).`
          : 'Todavía no enrolaste tu voz — hace falta para que Jarvis pueda reconocerte.'}
      </p>
      <p className={styles.hint}>Se va a grabar desde: {deviceLabel}</p>
      <button type="button" className={styles.recordButton} onClick={() => setActive(true)}>
        <Mic size={14} strokeWidth={2} />
        {status?.enrolled ? 'Volver a grabar' : `Grabar mi voz (${ENROLL_SECONDS}s)`}
      </button>
    </div>
  );
}

function ActiveEnrollment({ deviceIndex, onDone }: { deviceIndex: number | null; onDone: () => void }) {
  const { status, progress, error, cancel } = useVoiceEnrollment(deviceIndex, ENROLL_SECONDS);
  const percent = progress ? Math.min(100, (progress.elapsedMs / progress.totalMs) * 100) : 0;

  // El worker no manda ningún evento tras un `cancel_enroll` (una
  // grabación parcial no vale la pena procesarla) -- la UI vuelve sola al
  // estado inicial apenas el usuario cancela, sin esperar una respuesta
  // que nunca llega.
  const handleCancel = () => {
    cancel();
    onDone();
  };

  return (
    <div className={styles.panel}>
      {status === 'connecting' && <p className={styles.hint}>Conectando con Jarvis…</p>}

      {status === 'recording' && (
        <>
          <div className={styles.progressRow}>
            <div className={styles.progressTrack}>
              <div className={styles.progressFill} style={{ width: `${percent}%` }} />
            </div>
            <button type="button" className={styles.cancelButton} onClick={handleCancel} aria-label="Cancelar grabación">
              <X size={14} strokeWidth={2} />
            </button>
          </div>
          <p className={styles.hint}>Hablá normal, una o dos frases — grabando…</p>
        </>
      )}

      {status === 'processing' && (
        <p className={styles.hint}>
          Calculando tu voz de referencia… puede tardar hasta un minuto la primera vez (descarga el modelo).
        </p>
      )}

      {status === 'done' && (
        <>
          <div className={styles.resultRow}>
            <CheckCircle2 size={16} strokeWidth={2} className={styles.successIcon} />
            <span>Listo, guardamos tu voz de referencia.</span>
          </div>
          <button type="button" className={styles.recordButton} onClick={onDone}>
            Volver
          </button>
        </>
      )}

      {status === 'error' && (
        <>
          <div className={styles.errorRow}>
            <AlertTriangle size={16} strokeWidth={2} />
            <span>{error ?? 'Algo falló grabando tu voz.'}</span>
          </div>
          <button type="button" className={styles.recordButton} onClick={onDone}>
            Volver a intentar
          </button>
        </>
      )}
    </div>
  );
}
