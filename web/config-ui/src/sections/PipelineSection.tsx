import { api } from '../api/client';
import type { PipelineConfig } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { SaveBar } from '../components/SaveBar';
import { Field, FieldGroup } from '../components/form/Field';
import { Slider } from '../components/form/Controls';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';

export function PipelineSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const { value, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<PipelineConfig>({ load: api.getPipeline, save: api.putPipeline });

  const save = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Sección Pipeline guardada.' });
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    }
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const cfg = value;
  const set = <K extends keyof PipelineConfig>(key: K, val: PipelineConfig[K]) =>
    setValue((prev) => ({ ...prev, [key]: val }));

  return (
    <div>
      <PageHeader
        title="Pipeline"
        subtitle="Cómo se corta la respuesta del modelo en frases antes de mandarlas a sintetizar."
      />

      <FieldGroup>
        <Field
          label="Máximo de caracteres por frase"
          hint="Corta una frase para sintetizarla aunque el modelo no haya terminado."
        >
          <Slider
            value={cfg.max_phrase_chars}
            min={60}
            max={500}
            step={10}
            onChange={(v) => set('max_phrase_chars', v)}
          />
        </Field>
        <Field
          label="Mínimo de caracteres por frase"
          hint="Fragmentos más cortos se juntan con el siguiente antes de sintetizar."
        >
          <Slider
            value={cfg.min_phrase_chars}
            min={1}
            max={100}
            onChange={(v) => set('min_phrase_chars', v)}
          />
        </Field>
      </FieldGroup>

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={save} onDiscard={handleDiscard} />
    </div>
  );
}
