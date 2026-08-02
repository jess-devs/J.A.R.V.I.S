import { api } from '../api/client';
import type { SttConfig } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { SaveBar } from '../components/SaveBar';
import { FieldGroup } from '../components/form/Field';
import { DeviceCalibrationPanel } from '../components/calibration/DeviceCalibrationPanel';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';

/**
 * Sección "Micrófono" del sidebar: el mismo `DeviceCalibrationPanel` que
 * usa `OnboardingWizard` en el primer arranque, reutilizado acá para
 * recalibrar cuando se quiera (cambiaste de micrófono, te mudaste de
 * cuarto, etc.). Guarda directo en `stt.input_device_index` — no hay una
 * sección de config propia para esto, `onboarding.completed` es solo el
 * flag de "ya pasaste por el wizard alguna vez".
 */
export function OnboardingSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const { value, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<SttConfig>({ load: api.getStt, save: api.putStt });

  const save = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Micrófono guardado.' });
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    }
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const cfg = value;

  return (
    <div>
      <PageHeader
        title="Micrófono"
        subtitle="Elegí tu dispositivo de entrada y mirá el nivel en vivo para confirmar que Jarvis te escucha bien."
      />

      <FieldGroup title="Dispositivo" columns={1}>
        <DeviceCalibrationPanel
          selectedDeviceIndex={cfg.input_device_index}
          onSelect={(index) => setValue((prev) => ({ ...prev, input_device_index: index }))}
        />
      </FieldGroup>

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={save} onDiscard={handleDiscard} />
    </div>
  );
}
