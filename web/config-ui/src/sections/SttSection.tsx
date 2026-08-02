import { api } from '../api/client';
import type { SttConfig, SttEngine } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { SaveBar } from '../components/SaveBar';
import { Field, FieldGroup } from '../components/form/Field';
import {
  NumberInput,
  OptionalNumberInput,
  Select,
  Slider,
  TextArea,
  TextInput,
  Toggle,
} from '../components/form/Controls';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';

const ENGINE_OPTIONS: { value: SttEngine; label: string }[] = [
  { value: 'native', label: 'Nativo (PyAudio + Silero + faster-whisper)' },
  { value: 'realtimestt', label: 'RealtimeSTT (respaldo)' },
];

export function SttSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const { value, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<SttConfig>({ load: api.getStt, save: api.putStt });

  const save = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Sección Voz (STT) guardada.' });
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    }
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const cfg = value;
  const set = <K extends keyof SttConfig>(key: K, val: SttConfig[K]) =>
    setValue((prev) => ({ ...prev, [key]: val }));

  return (
    <div>
      <PageHeader
        title="Voz (STT)"
        subtitle="Reconocimiento de voz: motor, detección de habla, filtros y el detector de doble aplauso."
      />

      <FieldGroup title="Motor">
        <Field label="Motor de STT">
          <Select value={cfg.engine} onChange={(v) => set('engine', v as SttEngine)} options={ENGINE_OPTIONS} />
        </Field>
        <Field label="Idioma">
          <TextInput value={cfg.language} onChange={(v) => set('language', v)} />
        </Field>
        <Field label="Dispositivo de cómputo" hint="auto, cuda o cpu.">
          <TextInput monospace value={cfg.device} onChange={(v) => set('device', v)} />
        </Field>
        <Field label="Modelo Whisper" hint="auto calibra la velocidad real de tu máquina.">
          <TextInput monospace value={cfg.whisper_model} onChange={(v) => set('whisper_model', v)} />
        </Field>
        <Field label="Tipo de cómputo" hint="auto, float16, int8_float16, int8.">
          <TextInput monospace value={cfg.compute_type} onChange={(v) => set('compute_type', v)} />
        </Field>
        <Field label="Recalibrar al arrancar" hint="Ignora el caché y vuelve a medir.">
          <Toggle checked={cfg.recalibrate} onChange={(v) => set('recalibrate', v)} />
        </Field>
        <Field label="Índice de micrófono" hint="Vacío = micrófono por defecto.">
          <OptionalNumberInput
            value={cfg.input_device_index}
            defaultValue={0}
            min={0}
            onChange={(v) => set('input_device_index', v)}
          />
        </Field>
        <Field label="Beam size" hint="Vacío = automático.">
          <OptionalNumberInput value={cfg.beam_size} defaultValue={5} min={1} max={10} onChange={(v) => set('beam_size', v)} />
        </Field>
        <Field label="Hilos de CPU" hint="Vacío = automático (núcleos físicos).">
          <OptionalNumberInput value={cfg.cpu_threads} defaultValue={4} min={1} max={64} onChange={(v) => set('cpu_threads', v)} />
        </Field>
        <Field label="Timeout de worker trabado" hint="Aplica a ambos motores.">
          <NumberInput value={cfg.stuck_state_timeout_secs} min={5} max={120} suffix="s" onChange={(v) => set('stuck_state_timeout_secs', v)} />
        </Field>
      </FieldGroup>

      <FieldGroup title="Prompt inicial" columns={1}>
        <Field label="Contexto para el decoder de Whisper" hint="Mejora la precisión con nombres propios y contexto.">
          <TextArea value={cfg.initial_prompt} onChange={(v) => set('initial_prompt', v)} rows={3} />
        </Field>
      </FieldGroup>

      {cfg.engine === 'native' && (
        <>
          <FieldGroup title="Detección de voz (VAD)">
            <Field label="Umbral de inicio de voz">
              <Slider
                value={Math.round(cfg.vad.threshold * 100)}
                min={0}
                max={100}
                onChange={(v) => set('vad', { ...cfg.vad, threshold: v / 100 })}
                format={(v) => `${v}%`}
              />
            </Field>
            <Field label="Umbral de fin de voz" hint="Histéresis anti micro-pausas.">
              <Slider
                value={Math.round(cfg.vad.neg_threshold * 100)}
                min={0}
                max={100}
                onChange={(v) => set('vad', { ...cfg.vad, neg_threshold: v / 100 })}
                format={(v) => `${v}%`}
              />
            </Field>
            <Field label="Pre-roll" hint="Audio previo antepuesto al detectar voz.">
              <NumberInput value={cfg.vad.pre_roll_ms} min={0} max={2000} suffix="ms" onChange={(v) => set('vad', { ...cfg.vad, pre_roll_ms: v })} />
            </Field>
            <Field label="Voz mínima" hint="Filtra blips más cortos que esto.">
              <NumberInput value={cfg.vad.min_speech_ms} min={0} max={2000} suffix="ms" onChange={(v) => set('vad', { ...cfg.vad, min_speech_ms: v })} />
            </Field>
            <Field label="Silencio para cerrar frase (corta)">
              <NumberInput value={cfg.vad.silence_long_ms} min={0} max={3000} suffix="ms" onChange={(v) => set('vad', { ...cfg.vad, silence_long_ms: v })} />
            </Field>
            <Field label="Silencio para cerrar frase (larga)">
              <NumberInput value={cfg.vad.silence_short_ms} min={0} max={3000} suffix="ms" onChange={(v) => set('vad', { ...cfg.vad, silence_short_ms: v })} />
            </Field>
            <Field label="Umbral de frase larga">
              <NumberInput value={cfg.vad.long_utterance_ms} min={0} max={10000} suffix="ms" onChange={(v) => set('vad', { ...cfg.vad, long_utterance_ms: v })} />
            </Field>
            <Field label="Segundos de calibración de ambiente">
              <Slider value={cfg.vad.calibration_secs} min={0} max={5} step={0.1} onChange={(v) => set('vad', { ...cfg.vad, calibration_secs: v })} format={(v) => `${v}s`} />
            </Field>
            <Field label="Piso de energía" hint="Vacío = calibrar al arrancar.">
              <OptionalNumberInput
                value={cfg.vad.energy_floor_dbfs}
                defaultValue={-40}
                min={-80}
                max={0}
                suffix="dBFS"
                onChange={(v) => set('vad', { ...cfg.vad, energy_floor_dbfs: v })}
              />
            </Field>
          </FieldGroup>

          <FieldGroup title="Filtros anti-alucinación">
            <Field label="Máximo de probabilidad de no-habla">
              <Slider
                value={Math.round(cfg.filters.max_no_speech_prob * 100)}
                min={0}
                max={100}
                onChange={(v) => set('filters', { ...cfg.filters, max_no_speech_prob: v / 100 })}
                format={(v) => `${v}%`}
              />
            </Field>
            <Field label="Mínimo log-probability promedio">
              <Slider
                value={cfg.filters.min_avg_logprob}
                min={-3}
                max={0}
                step={0.05}
                onChange={(v) => set('filters', { ...cfg.filters, min_avg_logprob: v })}
                format={(v) => v.toFixed(2)}
              />
            </Field>
            <Field label="Máximo ratio de compresión" hint="Indicio de texto repetitivo/alucinado.">
              <Slider
                value={cfg.filters.max_compression_ratio}
                min={1}
                max={5}
                step={0.1}
                onChange={(v) => set('filters', { ...cfg.filters, max_compression_ratio: v })}
                format={(v) => v.toFixed(1)}
              />
            </Field>
          </FieldGroup>

          <FieldGroup title="Doble aplauso">
            <Field label="Pico mínimo">
              <NumberInput value={cfg.clap.min_peak_dbfs} min={-80} max={0} suffix="dBFS" onChange={(v) => set('clap', { ...cfg.clap, min_peak_dbfs: v })} />
            </Field>
            <Field label="Subida mínima sobre el fondo">
              <NumberInput value={cfg.clap.min_rise_db} min={0} max={40} suffix="dB" onChange={(v) => set('clap', { ...cfg.clap, min_rise_db: v })} />
            </Field>
            <Field label="Ventana de decaimiento">
              <NumberInput value={cfg.clap.decay_ms} min={0} max={1000} suffix="ms" onChange={(v) => set('clap', { ...cfg.clap, decay_ms: v })} />
            </Field>
            <Field label="Máxima probabilidad de voz">
              <Slider
                value={Math.round(cfg.clap.max_vad_prob * 100)}
                min={0}
                max={100}
                onChange={(v) => set('clap', { ...cfg.clap, max_vad_prob: v / 100 })}
                format={(v) => `${v}%`}
              />
            </Field>
            <Field label="Mínima tasa de cruces por cero" hint="Filtra clics de teclado/trackpad.">
              <Slider
                value={cfg.clap.min_zcr}
                min={0}
                max={1}
                step={0.01}
                onChange={(v) => set('clap', { ...cfg.clap, min_zcr: v })}
                format={(v) => v.toFixed(2)}
              />
            </Field>
            <Field label="Gap mínimo entre aplausos">
              <NumberInput value={cfg.clap.double_min_gap_ms} min={0} max={2000} suffix="ms" onChange={(v) => set('clap', { ...cfg.clap, double_min_gap_ms: v })} />
            </Field>
            <Field label="Gap máximo entre aplausos">
              <NumberInput value={cfg.clap.double_max_gap_ms} min={0} max={3000} suffix="ms" onChange={(v) => set('clap', { ...cfg.clap, double_max_gap_ms: v })} />
            </Field>
            <Field label="Refractario tras confirmar">
              <NumberInput value={cfg.clap.refractory_ms} min={0} max={5000} suffix="ms" onChange={(v) => set('clap', { ...cfg.clap, refractory_ms: v })} />
            </Field>
          </FieldGroup>
        </>
      )}

      {cfg.engine === 'realtimestt' && (
        <FieldGroup title="RealtimeSTT (motor de respaldo)">
          <Field label="Sensibilidad VAD Silero">
            <Slider
              value={Math.round(cfg.silero_sensitivity * 100)}
              min={0}
              max={100}
              onChange={(v) => set('silero_sensitivity', v / 100)}
              format={(v) => `${v}%`}
            />
          </Field>
          <Field label="Sensibilidad VAD WebRTC">
            <NumberInput value={cfg.webrtc_sensitivity} min={0} max={3} onChange={(v) => set('webrtc_sensitivity', v)} />
          </Field>
          <Field label="Silencio antes de cerrar frase">
            <Slider value={cfg.post_speech_silence_duration} min={0} max={3} step={0.1} onChange={(v) => set('post_speech_silence_duration', v)} format={(v) => `${v}s`} />
          </Field>
          <Field label="Grabación mínima para contar como habla">
            <Slider value={cfg.min_length_of_recording} min={0} max={5} step={0.1} onChange={(v) => set('min_length_of_recording', v)} format={(v) => `${v}s`} />
          </Field>
          <Field label="Mínimo entre grabaciones">
            <Slider value={cfg.min_gap_between_recordings} min={0} max={5} step={0.1} onChange={(v) => set('min_gap_between_recordings', v)} format={(v) => `${v}s`} />
          </Field>
          <Field label="Usar Silero para detectar fin del habla">
            <Toggle checked={cfg.silero_deactivity_detection} onChange={(v) => set('silero_deactivity_detection', v)} />
          </Field>
        </FieldGroup>
      )}

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={save} onDiscard={handleDiscard} />
    </div>
  );
}
