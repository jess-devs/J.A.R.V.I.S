import { useEffect, useState } from 'react';
import { api } from '../api/client';
import type { LlmConfig, LlmProvider, LlmStatus } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { StatusCard } from '../components/StatusCard';
import { SaveBar } from '../components/SaveBar';
import { Field, FieldGroup } from '../components/form/Field';
import fieldStyles from '../components/form/Field.module.css';
import { Select, Slider, TextInput, TextArea, Toggle } from '../components/form/Controls';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';

const PROVIDER_OPTIONS: { value: LlmProvider; label: string }[] = [
  { value: 'ollama', label: 'Ollama (local)' },
  { value: 'lmstudio', label: 'LM Studio (local)' },
  { value: 'anthropic', label: 'Anthropic' },
  { value: 'openai', label: 'OpenAI' },
  { value: 'deepseek', label: 'DeepSeek' },
];

export function LlmSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const { value, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<LlmConfig>({ load: api.getLlm, save: api.putLlm });

  const [status, setStatus] = useState<LlmStatus | null>(null);

  const refreshStatus = () => {
    api
      .statusLlm()
      .then(setStatus)
      .catch(() => setStatus(null));
  };

  useEffect(refreshStatus, [value?.provider]);

  const save = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Sección LLM guardada.' });
      refreshStatus();
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    }
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const cfg = value;
  const set = <K extends keyof LlmConfig>(key: K, val: LlmConfig[K]) =>
    setValue((prev) => ({ ...prev, [key]: val }));

  return (
    <div>
      <PageHeader
        title="Modelo de lenguaje"
        subtitle="Elegí qué modelo piensa las respuestas de Jarvis y cómo se comporta."
      />

      {status && (
        <div style={{ display: 'flex', gap: 12, marginBottom: 8 }}>
          <StatusCard
            label={statusLabel(cfg.provider)}
            ok={status.reachable ?? status.api_key_present}
            okText={status.reachable !== null ? 'Conectado' : 'Definida'}
            badText={status.reachable !== null ? 'No conectado' : 'Falta'}
            detail={status.detail}
          />
        </div>
      )}

      <FieldGroup title="Proveedor">
        <Field label="Proveedor de LLM">
          <Select
            value={cfg.provider}
            onChange={(v) => set('provider', v as LlmProvider)}
            options={PROVIDER_OPTIONS}
          />
        </Field>
      </FieldGroup>

      {cfg.provider === 'ollama' && (
        <FieldGroup title="Ollama">
          <Field label="URL base">
            <TextInput monospace value={cfg.ollama.base_url} onChange={(v) => set('ollama', { ...cfg.ollama, base_url: v })} />
          </Field>
          <Field label="Modelo" hint='"auto" detecta VRAM/RAM y elige un modelo qwen acorde.'>
            <TextInput monospace value={cfg.ollama.model} onChange={(v) => set('ollama', { ...cfg.ollama, model: v })} />
          </Field>
          <Field label="Levantar ollama serve automáticamente">
            <Toggle checked={cfg.ollama.auto_serve} onChange={(v) => set('ollama', { ...cfg.ollama, auto_serve: v })} />
          </Field>
          <Field label="Modelo de visión" hint='Usado solo cuando el turno lleva imágenes. Vacío = seguir usando el modelo de arriba.'>
            <TextInput
              monospace
              placeholder="ej. qwen2.5vl"
              value={cfg.ollama.vision_model ?? ''}
              onChange={(v) => set('ollama', { ...cfg.ollama, vision_model: v === '' ? null : v })}
            />
          </Field>
        </FieldGroup>
      )}

      {cfg.provider === 'lmstudio' && (
        <FieldGroup title="LM Studio">
          <Field label="URL base">
            <TextInput
              monospace
              value={cfg.lmstudio.base_url}
              onChange={(v) => set('lmstudio', { ...cfg.lmstudio, base_url: v })}
            />
          </Field>
          <Field label="Model identifier" hint="Exacto como aparece en la pestaña Developer de LM Studio.">
            <TextInput monospace value={cfg.lmstudio.model} onChange={(v) => set('lmstudio', { ...cfg.lmstudio, model: v })} />
          </Field>
          <Field label="Variable de entorno de API key" hint="Vacío = sin autenticación (default de LM Studio).">
            <TextInput
              monospace
              placeholder="ej. LMSTUDIO_API_KEY"
              value={cfg.lmstudio.api_key_env ?? ''}
              onChange={(v) => set('lmstudio', { ...cfg.lmstudio, api_key_env: v === '' ? null : v })}
            />
          </Field>
        </FieldGroup>
      )}

      {cfg.provider === 'anthropic' && (
        <FieldGroup title="Anthropic">
          <Field label="Modelo">
            <TextInput monospace value={cfg.anthropic.model} onChange={(v) => set('anthropic', { ...cfg.anthropic, model: v })} />
          </Field>
          <Field label="Variable de entorno de API key" hint="El valor real vive en tu .env, nunca acá.">
            <TextInput
              monospace
              value={cfg.anthropic.api_key_env}
              onChange={(v) => set('anthropic', { ...cfg.anthropic, api_key_env: v })}
            />
          </Field>
        </FieldGroup>
      )}

      {cfg.provider === 'openai' && (
        <FieldGroup title="OpenAI">
          <Field label="Modelo">
            <TextInput monospace value={cfg.openai.model} onChange={(v) => set('openai', { ...cfg.openai, model: v })} />
          </Field>
          <Field label="Variable de entorno de API key" hint="El valor real vive en tu .env, nunca acá.">
            <TextInput
              monospace
              value={cfg.openai.api_key_env}
              onChange={(v) => set('openai', { ...cfg.openai, api_key_env: v })}
            />
          </Field>
        </FieldGroup>
      )}

      {cfg.provider === 'deepseek' && (
        <FieldGroup title="DeepSeek">
          <Field label="Modelo">
            <TextInput monospace value={cfg.deepseek.model} onChange={(v) => set('deepseek', { ...cfg.deepseek, model: v })} />
          </Field>
          <Field label="Variable de entorno de API key" hint="El valor real vive en tu .env, nunca acá.">
            <TextInput
              monospace
              value={cfg.deepseek.api_key_env}
              onChange={(v) => set('deepseek', { ...cfg.deepseek, api_key_env: v })}
            />
          </Field>
        </FieldGroup>
      )}

      <FieldGroup title="Comportamiento" columns={1}>
        <Field label="Prompt de sistema" hint="Define la personalidad y el comportamiento de Jarvis.">
          <TextArea value={cfg.system_prompt} onChange={(v) => set('system_prompt', v)} rows={8} />
        </Field>
        <div className={fieldStyles.grid2}>
          <Field label="Mensajes de historial conservados">
            <Slider value={cfg.max_history_messages} min={2} max={60} onChange={(v) => set('max_history_messages', v)} leftLabel="Menos contexto" rightLabel="Más contexto" />
          </Field>
          <Field label="Timeout por solicitud">
            <Slider
              value={cfg.request_timeout_secs}
              min={10}
              max={300}
              step={5}
              onChange={(v) => set('request_timeout_secs', v)}
              format={(v) => `${v}s`}
            />
          </Field>
        </div>
      </FieldGroup>

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={save} onDiscard={handleDiscard} />
    </div>
  );
}

function statusLabel(provider: LlmProvider): string {
  switch (provider) {
    case 'ollama':
      return 'Ollama';
    case 'lmstudio':
      return 'LM Studio';
    case 'anthropic':
      return 'Anthropic API key';
    case 'openai':
      return 'OpenAI API key';
    case 'deepseek':
      return 'DeepSeek API key';
  }
}
