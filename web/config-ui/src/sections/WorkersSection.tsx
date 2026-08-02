import { api } from '../api/client';
import type { WorkersConfig } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { SaveBar } from '../components/SaveBar';
import { Field, FieldGroup } from '../components/form/Field';
import { NumberInput, Slider, TextInput, Toggle } from '../components/form/Controls';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';

export function WorkersSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const { value, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<WorkersConfig>({ load: api.getWorkers, save: api.putWorkers });

  const save = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Sección Workers guardada.' });
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    }
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const cfg = value;
  const set = <K extends keyof WorkersConfig>(key: K, val: WorkersConfig[K]) =>
    setValue((prev) => ({ ...prev, [key]: val }));

  return (
    <div>
      <PageHeader
        title="Workers"
        subtitle="Los procesos hijo de Python que hacen STT y TTS, y cómo Jarvis los supervisa."
      />

      <FieldGroup title="Ejecutables">
        <Field label="Python del venv" hint="Ruta al ejecutable dentro de workers/.venv.">
          <TextInput monospace value={cfg.python_executable} onChange={(v) => set('python_executable', v)} />
        </Field>
        <div />
        <Field label="Script de STT">
          <TextInput monospace value={cfg.stt_script} onChange={(v) => set('stt_script', v)} />
        </Field>
        <Field label="Script de TTS">
          <TextInput monospace value={cfg.tts_script} onChange={(v) => set('tts_script', v)} />
        </Field>
      </FieldGroup>

      <FieldGroup title="Tiempos de espera">
        <Field label="Timeout de carga de STT">
          <Slider
            value={cfg.stt_init_timeout_secs}
            min={10}
            max={180}
            step={5}
            onChange={(v) => set('stt_init_timeout_secs', v)}
            format={(v) => `${v}s`}
          />
        </Field>
        <Field label="Timeout de carga de TTS">
          <Slider
            value={cfg.tts_init_timeout_secs}
            min={5}
            max={90}
            step={5}
            onChange={(v) => set('tts_init_timeout_secs', v)}
            format={(v) => `${v}s`}
          />
        </Field>
        <Field label="Timeout de apagado" hint="Espera antes de matar un worker a la fuerza.">
          <Slider
            value={cfg.shutdown_timeout_secs}
            min={1}
            max={30}
            onChange={(v) => set('shutdown_timeout_secs', v)}
            format={(v) => `${v}s`}
          />
        </Field>
      </FieldGroup>

      <FieldGroup title="Reinicio ante crash">
        <Field label="Reiniciar worker solo, si muere">
          <Toggle checked={cfg.restart_on_crash} onChange={(v) => set('restart_on_crash', v)} />
        </Field>
        <Field label="Máximo de reinicios" hint="Al agotarse, Jarvis termina.">
          <NumberInput
            value={cfg.max_restarts}
            min={0}
            max={20}
            onChange={(v) => set('max_restarts', v)}
          />
        </Field>
      </FieldGroup>

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={save} onDiscard={handleDiscard} />
    </div>
  );
}
