import { useCallback, useEffect, useRef, useState } from 'react';
import type { CalibrationServerMessage } from '../api/types';

export type EnrollmentStatus = 'connecting' | 'recording' | 'processing' | 'done' | 'error';

interface Progress {
  elapsedMs: number;
  totalMs: number;
}

interface UseVoiceEnrollmentResult {
  status: EnrollmentStatus;
  progress: Progress | null;
  error: string | null;
  embeddingPath: string | null;
  cancel: () => void;
}

function send(socket: WebSocket | null, msg: unknown) {
  if (socket && socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify(msg));
  }
}

/**
 * Abre el WebSocket de calibración/enrollment y arranca a grabar apenas
 * conecta (mismo endpoint que `useCalibrationSocket`, mismo patrón de
 * worker bajo demanda por conexión — ver `src/config_ui/calibration.rs`).
 * Solo se monta cuando el usuario ya apretó "grabar" (ver
 * `EnrollmentPanel`) — entrar a la sección "Micrófono" no activa esto solo.
 *
 * Durante la grabación el worker NO manda `level` (ver
 * `CalibrationEngine.is_enrolling` del lado Python: leer el stream desde
 * dos hilos a la vez se robaría frames entre sí), así que acá no hay un
 * medidor de nivel en vivo — el feedback es el progreso de tiempo
 * (`enroll_progress`), no un VU meter.
 */
export function useVoiceEnrollment(deviceIndex: number | null, seconds = 5): UseVoiceEnrollmentResult {
  const socketRef = useRef<WebSocket | null>(null);
  const [status, setStatus] = useState<EnrollmentStatus>('connecting');
  const [progress, setProgress] = useState<Progress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [embeddingPath, setEmbeddingPath] = useState<string | null>(null);

  useEffect(() => {
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const socket = new WebSocket(`${protocol}//${location.host}/api/onboarding/calibration/ws`);
    socketRef.current = socket;

    socket.onopen = () => {
      send(socket, { type: 'start_enroll', device_index: deviceIndex, seconds });
    };

    socket.onerror = () => {
      setStatus('error');
      setError('No se pudo conectar con Jarvis para grabar tu voz.');
    };

    socket.onclose = () => {
      // Si se cerró sin haber llegado a un estado final, fue una
      // desconexión inesperada -- lo reflejamos como error en vez de
      // dejar la UI congelada en "recording"/"processing" para siempre.
      setStatus((prev) => (prev === 'done' || prev === 'error' ? prev : 'error'));
    };

    socket.onmessage = (event: MessageEvent<string>) => {
      let msg: CalibrationServerMessage;
      try {
        msg = JSON.parse(event.data) as CalibrationServerMessage;
      } catch {
        return;
      }
      switch (msg.type) {
        case 'enroll_started':
          setStatus('recording');
          setProgress({ elapsedMs: 0, totalMs: msg.total_ms });
          break;
        case 'enroll_progress':
          setStatus('recording');
          setProgress({ elapsedMs: msg.elapsed_ms, totalMs: msg.total_ms });
          break;
        case 'enroll_processing':
          setStatus('processing');
          break;
        case 'enroll_complete':
          setStatus('done');
          setEmbeddingPath(msg.embedding_path);
          break;
        case 'error':
          setStatus('error');
          setError(msg.message);
          break;
        case 'died':
          setStatus('error');
          setError('El proceso de grabación se detuvo inesperadamente.');
          break;
        default:
          break;
      }
    };

    return () => {
      socket.close();
      socketRef.current = null;
    };
    // Solo al montar/desmontar -- `deviceIndex`/`seconds` son el snapshot
    // con el que arrancó ESTA grabación, cambiarlos no debe reabrir el WS.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const cancel = useCallback(() => {
    send(socketRef.current, { type: 'cancel_enroll' });
  }, []);

  return { status, progress, error, embeddingPath, cancel };
}
