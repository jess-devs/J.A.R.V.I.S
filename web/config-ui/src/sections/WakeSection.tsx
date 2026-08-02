import { api } from '../api/client';
import type { WakeConfig } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { SaveBar } from '../components/SaveBar';
import { Field, FieldGroup } from '../components/form/Field';
import { NumberInput, StringListEditor, Toggle } from '../components/form/Controls';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';

export function WakeSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const { value, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<WakeConfig>({ load: api.getWake, save: api.putWake });

  const save = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Sección Palabra de activación guardada.' });
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    }
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const cfg = value;
  const set = <K extends keyof WakeConfig>(key: K, val: WakeConfig[K]) =>
    setValue((prev) => ({ ...prev, [key]: val }));

  return (
    <div>
      <PageHeader
        title="Palabra de activación"
        subtitle="Qué nombre despierta a Jarvis y cuánto sigue atento después de responder."
      />

      <FieldGroup title="Activación">
        <Field label="Gate de activación">
          <Toggle checked={cfg.enabled} onChange={(v) => set('enabled', v)} />
        </Field>
        <div />
        <Field label="Palabras de activación" hint="Con tolerancia a 1 letra de error de transcripción.">
          <StringListEditor values={cfg.words} onChange={(v) => set('words', v)} placeholder="ej. jarvis" />
        </Field>
      </FieldGroup>

      <FieldGroup title="Ventana de atención">
        <Field label="Segundos atento sin repetir el nombre">
          <NumberInput value={cfg.attention_window_secs} min={0} max={120} suffix="s" onChange={(v) => set('attention_window_secs', v)} />
        </Field>
        <Field label="Mínimo de palabras dentro de la ventana" hint="1 = sin filtro.">
          <NumberInput value={cfg.window_min_words} min={1} max={10} onChange={(v) => set('window_min_words', v)} />
        </Field>
      </FieldGroup>

      <FieldGroup title="Contexto ambiente">
        <Field label="Pasar frases ignoradas como contexto">
          <Toggle checked={cfg.ambient_context} onChange={(v) => set('ambient_context', v)} />
        </Field>
        <div />
        <Field label="Máximo de frases conservadas">
          <NumberInput value={cfg.ambient_context_max} min={0} max={30} onChange={(v) => set('ambient_context_max', v)} />
        </Field>
        <Field label="Vencen después de">
          <NumberInput value={cfg.ambient_context_ttl_secs} min={0} max={600} suffix="s" onChange={(v) => set('ambient_context_ttl_secs', v)} />
        </Field>
      </FieldGroup>

      <FieldGroup title="Frases-basura" columns={1}>
        <Field
          label="Frases que Whisper inventa en silencio"
          hint="Si la transcripción normalizada coincide, se descarta por completo."
        >
          <StringListEditor
            values={cfg.ignore_phrases}
            onChange={(v) => set('ignore_phrases', v)}
            placeholder="ej. gracias por ver el video"
          />
        </Field>
      </FieldGroup>

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={save} onDiscard={handleDiscard} />
    </div>
  );
}
