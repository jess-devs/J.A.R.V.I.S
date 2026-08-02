import { api } from '../api/client';
import type { AudioConfig } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { SaveBar } from '../components/SaveBar';
import { Field, FieldGroup } from '../components/form/Field';
import { Slider, TextInput } from '../components/form/Controls';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';

export function AudioSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const { value, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<AudioConfig>({ load: api.getAudio, save: api.putAudio });

  const save = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Sección Audio guardada.' });
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    }
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const cfg = value;
  const set = <K extends keyof AudioConfig>(key: K, val: AudioConfig[K]) =>
    setValue((prev) => ({ ...prev, [key]: val }));

  return (
    <div>
      <PageHeader title="Audio" subtitle="Salida de audio: dispositivo, volumen y tolerancia a cuelgues." />

      <FieldGroup>
        <Field label="Dispositivo de salida" hint="Vacío = dispositivo por defecto del sistema.">
          <TextInput
            monospace
            placeholder="por defecto"
            value={cfg.output_device ?? ''}
            onChange={(v) => set('output_device', v === '' ? null : v)}
          />
        </Field>
        <Field label="Volumen global">
          <Slider
            value={Math.round(cfg.volume * 100)}
            min={0}
            max={100}
            onChange={(v) => set('volume', v / 100)}
            format={(v) => `${v}%`}
          />
        </Field>
        <Field
          label="Timeout de drenado"
          hint="Límite esperando a que termine de sonar una respuesta si el dispositivo se cuelga."
        >
          <Slider
            value={cfg.drain_timeout_secs}
            min={5}
            max={180}
            step={5}
            onChange={(v) => set('drain_timeout_secs', v)}
            format={(v) => `${v}s`}
          />
        </Field>
      </FieldGroup>

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={save} onDiscard={handleDiscard} />
    </div>
  );
}
