import { api } from '../api/client';
import type { BargeInConfig, BargeInMode } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { SaveBar } from '../components/SaveBar';
import { Field, FieldGroup } from '../components/form/Field';
import { NumberInput, Select, Slider, Toggle } from '../components/form/Controls';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';

const MODE_OPTIONS: { value: BargeInMode; label: string }[] = [
  { value: 'wake_word', label: 'Wake word (con altavoces)' },
  { value: 'any_voice', label: 'Cualquier voz (con auriculares)' },
];

export function BargeInSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const { value, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<BargeInConfig>({ load: api.getBargeIn, save: api.putBargeIn });

  const save = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Sección Interrupción guardada.' });
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    }
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const cfg = value;
  const set = <K extends keyof BargeInConfig>(key: K, val: BargeInConfig[K]) =>
    setValue((prev) => ({ ...prev, [key]: val }));

  return (
    <div>
      <PageHeader
        title="Interrupción"
        subtitle="Si y cómo se puede cortar a Jarvis mientras habla."
      />

      <FieldGroup title="Modo">
        <Field label="Permitir interrumpir a Jarvis">
          <Toggle checked={cfg.enabled} onChange={(v) => set('enabled', v)} />
        </Field>
        <Field label="Modo de interrupción" hint="Solo aplica con stt.engine: native.">
          <Select value={cfg.mode} onChange={(v) => set('mode', v as BargeInMode)} options={MODE_OPTIONS} />
        </Field>
        <Field label="Voz sostenida mínima para confirmar">
          <Slider value={cfg.min_speech_ms} min={100} max={2000} step={50} onChange={(v) => set('min_speech_ms', v)} format={(v) => `${v}ms`} />
        </Field>
        <Field label="Timeout de relevancia" hint="Solo modo any_voice: si el LLM tarda más, Jarvis sigue hablando.">
          <NumberInput value={cfg.relevance_timeout_secs} min={1} max={15} suffix="s" onChange={(v) => set('relevance_timeout_secs', v)} />
        </Field>
      </FieldGroup>

      <FieldGroup title="Echo guard" columns={2}>
        <Field label="Evitar autointerrupción por eco">
          <Toggle checked={cfg.echo_guard.enabled} onChange={(v) => set('echo_guard', { ...cfg.echo_guard, enabled: v })} />
        </Field>
        <div />
        <Field label="Umbral de solapamiento con lo dicho">
          <Slider
            value={Math.round(cfg.echo_guard.similarity_threshold * 100)}
            min={0}
            max={100}
            onChange={(v) => set('echo_guard', { ...cfg.echo_guard, similarity_threshold: v / 100 })}
            format={(v) => `${v}%`}
          />
        </Field>
        <Field label="Umbral de voz mientras Jarvis habla">
          <Slider
            value={Math.round(cfg.echo_guard.vad_threshold_while_speaking * 100)}
            min={0}
            max={100}
            onChange={(v) => set('echo_guard', { ...cfg.echo_guard, vad_threshold_while_speaking: v / 100 })}
            format={(v) => `${v}%`}
          />
        </Field>
        <Field label="Ventana de frases recientes">
          <NumberInput
            value={cfg.echo_guard.recent_tts_window_secs}
            min={1}
            max={60}
            suffix="s"
            onChange={(v) => set('echo_guard', { ...cfg.echo_guard, recent_tts_window_secs: v })}
          />
        </Field>
      </FieldGroup>

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={save} onDiscard={handleDiscard} />
    </div>
  );
}
