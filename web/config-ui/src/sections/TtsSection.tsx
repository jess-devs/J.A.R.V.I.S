import { useEffect, useState } from 'react';
import { api } from '../api/client';
import type { CartesiaTransport, TtsConfig, TtsProvider, TtsStatus } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { StatusCard } from '../components/StatusCard';
import { SaveBar } from '../components/SaveBar';
import { Field, FieldGroup } from '../components/form/Field';
import { OptionalSlider, Select, Slider, TextInput, Toggle } from '../components/form/Controls';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';

const PROVIDER_OPTIONS: { value: TtsProvider; label: string }[] = [
  { value: 'piper', label: 'Piper (local)' },
  { value: 'elevenlabs', label: 'ElevenLabs' },
  { value: 'cartesia', label: 'Cartesia' },
];

const TRANSPORT_OPTIONS: { value: CartesiaTransport; label: string }[] = [
  { value: 'websocket', label: 'WebSocket (menor latencia)' },
  { value: 'rest', label: 'REST' },
];

export function TtsSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const { value, loading, error, saving, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<TtsConfig>({ load: api.getTts, save: api.putTts });

  const [status, setStatus] = useState<TtsStatus | null>(null);

  const refreshStatus = () => {
    api
      .statusTts()
      .then(setStatus)
      .catch(() => setStatus(null));
  };

  useEffect(refreshStatus, [value?.provider]);

  const save = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Sección TTS guardada.' });
      refreshStatus();
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    }
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const cfg = value;
  const set = <K extends keyof TtsConfig>(key: K, val: TtsConfig[K]) =>
    setValue((prev) => ({ ...prev, [key]: val }));

  return (
    <div>
      <PageHeader title="Síntesis de voz" subtitle="Elegí con qué voz habla Jarvis y cómo suena." />

      {status && (
        <div style={{ display: 'flex', gap: 12, marginBottom: 8 }}>
          <StatusCard
            label={statusLabel(cfg.provider)}
            ok={status.voice_files_present ?? status.api_key_present}
            okText={status.voice_files_present !== null ? 'Voz encontrada' : 'Definida'}
            badText={status.voice_files_present !== null ? 'Archivos faltantes' : 'Falta'}
            detail={status.detail}
          />
        </div>
      )}

      <FieldGroup title="Proveedor">
        <Field label="Proveedor de TTS">
          <Select value={cfg.provider} onChange={(v) => set('provider', v as TtsProvider)} options={PROVIDER_OPTIONS} />
        </Field>
        <Field label="Timeout de síntesis por frase">
          <Slider
            value={cfg.synth_timeout_secs}
            min={3}
            max={60}
            onChange={(v) => set('synth_timeout_secs', v)}
            format={(v) => `${v}s`}
          />
        </Field>
      </FieldGroup>

      {cfg.provider === 'piper' && (
        <FieldGroup title="Piper">
          <Field label="Ruta de la voz (.onnx)">
            <TextInput monospace value={cfg.piper.voice_path} onChange={(v) => set('piper', { ...cfg.piper, voice_path: v })} />
          </Field>
          <Field label="Ruta de configuración (.onnx.json)">
            <TextInput
              monospace
              value={cfg.piper.config_path}
              onChange={(v) => set('piper', { ...cfg.piper, config_path: v })}
            />
          </Field>
          <Field label="Sintetizar en GPU (CUDA)">
            <Toggle checked={cfg.piper.use_cuda} onChange={(v) => set('piper', { ...cfg.piper, use_cuda: v })} />
          </Field>
          <div />
          <Field label="Velocidad" hint="Menos de 1 = más rápido, más de 1 = más lento.">
            <OptionalSlider
              value={cfg.piper.length_scale}
              defaultValue={1}
              min={0.5}
              max={1.8}
              step={0.05}
              onChange={(v) => set('piper', { ...cfg.piper, length_scale: v })}
              format={(v) => v.toFixed(2)}
            />
          </Field>
          <Field label="Variación de fonemas" hint="Un poco más que el default suena menos robótico.">
            <OptionalSlider
              value={cfg.piper.noise_w_scale}
              defaultValue={0.8}
              min={0}
              max={1.5}
              step={0.05}
              onChange={(v) => set('piper', { ...cfg.piper, noise_w_scale: v })}
              format={(v) => v.toFixed(2)}
            />
          </Field>
        </FieldGroup>
      )}

      {cfg.provider === 'elevenlabs' && (
        <FieldGroup title="ElevenLabs">
          <Field label="ID de voz">
            <TextInput
              monospace
              value={cfg.elevenlabs.voice_id}
              onChange={(v) => set('elevenlabs', { ...cfg.elevenlabs, voice_id: v })}
            />
          </Field>
          <Field label="ID de modelo">
            <TextInput
              monospace
              value={cfg.elevenlabs.model_id}
              onChange={(v) => set('elevenlabs', { ...cfg.elevenlabs, model_id: v })}
            />
          </Field>
          <Field label="Formato de salida" hint='Debe ser "pcm_*" — evita depender de ffmpeg.'>
            <TextInput
              monospace
              value={cfg.elevenlabs.output_format}
              onChange={(v) => set('elevenlabs', { ...cfg.elevenlabs, output_format: v })}
            />
          </Field>
          <Field label="Variable de entorno de API key" hint="El valor real vive en tu .env, nunca acá.">
            <TextInput
              monospace
              value={cfg.elevenlabs.api_key_env}
              onChange={(v) => set('elevenlabs', { ...cfg.elevenlabs, api_key_env: v })}
            />
          </Field>
        </FieldGroup>
      )}

      {cfg.provider === 'cartesia' && (
        <FieldGroup title="Cartesia">
          <Field label="ID de modelo">
            <TextInput
              monospace
              value={cfg.cartesia.model_id}
              onChange={(v) => set('cartesia', { ...cfg.cartesia, model_id: v })}
            />
          </Field>
          <Field label="ID de voz">
            <TextInput
              monospace
              value={cfg.cartesia.voice_id}
              onChange={(v) => set('cartesia', { ...cfg.cartesia, voice_id: v })}
            />
          </Field>
          <Field label="Idioma" hint="Vacío = autodetectar.">
            <TextInput
              monospace
              value={cfg.cartesia.language ?? ''}
              onChange={(v) => set('cartesia', { ...cfg.cartesia, language: v === '' ? null : v })}
            />
          </Field>
          <Field label="Transporte">
            <Select
              value={cfg.cartesia.transport}
              onChange={(v) => set('cartesia', { ...cfg.cartesia, transport: v as CartesiaTransport })}
              options={TRANSPORT_OPTIONS}
            />
          </Field>
          <Field label="Contenedor de salida">
            <TextInput
              monospace
              value={cfg.cartesia.output_format.container}
              onChange={(v) =>
                set('cartesia', { ...cfg.cartesia, output_format: { ...cfg.cartesia.output_format, container: v } })
              }
            />
          </Field>
          <Field label="Codificación de salida">
            <TextInput
              monospace
              value={cfg.cartesia.output_format.encoding}
              onChange={(v) =>
                set('cartesia', { ...cfg.cartesia, output_format: { ...cfg.cartesia.output_format, encoding: v } })
              }
            />
          </Field>
          <Field label="Frecuencia de muestreo (Hz)">
            <Slider
              value={cfg.cartesia.output_format.sample_rate}
              min={8000}
              max={48000}
              step={1000}
              onChange={(v) =>
                set('cartesia', { ...cfg.cartesia, output_format: { ...cfg.cartesia.output_format, sample_rate: v } })
              }
            />
          </Field>
          <Field label="Variable de entorno de API key" hint="El valor real vive en tu .env, nunca acá.">
            <TextInput
              monospace
              value={cfg.cartesia.api_key_env}
              onChange={(v) => set('cartesia', { ...cfg.cartesia, api_key_env: v })}
            />
          </Field>
        </FieldGroup>
      )}

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={save} onDiscard={handleDiscard} />
    </div>
  );
}

function statusLabel(provider: TtsProvider): string {
  switch (provider) {
    case 'piper':
      return 'Voz Piper';
    case 'elevenlabs':
      return 'ElevenLabs API key';
    case 'cartesia':
      return 'Cartesia API key';
  }
}
