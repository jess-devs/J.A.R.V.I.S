import { useState } from 'react';
import { Mic, SkipForward } from 'lucide-react';
import { api } from '../api/client';
import type { SttConfig } from '../api/types';
import { DeviceCalibrationPanel } from '../components/calibration/DeviceCalibrationPanel';
import { useConfigSection } from '../hooks/useConfigSection';
import styles from './OnboardingWizard.module.css';

type Step = 'intro' | 'calibrate';

/**
 * Pantalla de primer arranque, mostrada por `App.tsx` en vez del shell
 * normal mientras `onboarding.completed` sea `false` (ver
 * `docs/planning/onboarding-mic-calibracion.md`, Fase 7). El panel de
 * calibración es el mismo componente que reutiliza `OnboardingSection` en
 * el sidebar — acá solo se agrega el flujo de pasos y el flag final.
 */
export function OnboardingWizard({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState<Step>('intro');
  const [finishing, setFinishing] = useState(false);
  const { value, setValue, handleSave } = useConfigSection<SttConfig>({ load: api.getStt, save: api.putStt });

  const finish = async (saveDevice: boolean) => {
    setFinishing(true);
    try {
      if (saveDevice) {
        await handleSave();
      }
      await api.putOnboarding({ completed: true });
    } catch {
      // El wizard es puro valor agregado de UX, no un gate: si el guardado
      // falla (ej. Jarvis se reinició justo ahora), igual se deja pasar al
      // panel normal — el usuario puede reintentar desde "Micrófono" en el
      // sidebar en vez de quedar trabado acá.
    } finally {
      setFinishing(false);
      onDone();
    }
  };

  return (
    <div className={styles.overlay}>
      <div className={styles.card}>
        {step === 'intro' && (
          <>
            <Mic size={32} strokeWidth={1.5} className={styles.icon} />
            <h1 className={styles.title}>Bienvenido a Jarvis</h1>
            <p className={styles.body}>
              Antes de arrancar, elegí el micrófono que vas a usar. Vas a ver un medidor de nivel en
              vivo para confirmar que Jarvis te escucha bien — podés cambiarlo cuando quieras después
              desde "Micrófono" en el panel de configuración.
            </p>
            <div className={styles.actions}>
              <button type="button" className={styles.secondary} onClick={() => finish(false)} disabled={finishing}>
                <SkipForward size={14} strokeWidth={2} />
                Saltar por ahora
              </button>
              <button type="button" className={styles.primary} onClick={() => setStep('calibrate')}>
                Elegir micrófono
              </button>
            </div>
          </>
        )}

        {step === 'calibrate' && !value && <p className={styles.body}>Cargando…</p>}

        {step === 'calibrate' && value && (
          <>
            <h1 className={styles.title}>Elegí tu micrófono</h1>
            <p className={styles.body}>Hablá normal y mirá si la barra reacciona a tu voz.</p>
            <DeviceCalibrationPanel
              selectedDeviceIndex={value.input_device_index}
              onSelect={(index) => setValue((prev) => ({ ...prev, input_device_index: index }))}
            />
            <div className={styles.actions}>
              <button type="button" className={styles.secondary} onClick={() => finish(false)} disabled={finishing}>
                Saltar por ahora
              </button>
              <button type="button" className={styles.primary} onClick={() => finish(true)} disabled={finishing}>
                {finishing ? 'Guardando…' : 'Usar este micrófono'}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
