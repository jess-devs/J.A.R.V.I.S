import { useEffect, useState } from 'react';
import { api } from '../api/client';
import type { WelcomeConfig, WelcomeStatus } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { StatusCard } from '../components/StatusCard';
import { SaveBar } from '../components/SaveBar';
import { Field, FieldGroup } from '../components/form/Field';
import { Slider, TextInput, Toggle } from '../components/form/Controls';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';

export function WelcomeSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const { value, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<WelcomeConfig>({ load: api.getWelcome, save: api.putWelcome });

  const [status, setStatus] = useState<WelcomeStatus | null>(null);

  const refreshStatus = () => {
    api
      .statusWelcome()
      .then(setStatus)
      .catch(() => setStatus(null));
  };

  useEffect(refreshStatus, [value?.music_path]);

  const save = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Sección Bienvenida guardada.' });
      refreshStatus();
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    }
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const cfg = value;
  const set = <K extends keyof WelcomeConfig>(key: K, val: WelcomeConfig[K]) =>
    setValue((prev) => ({ ...prev, [key]: val }));

  return (
    <div>
      <PageHeader
        title="Bienvenida"
        subtitle="La escena de doble aplauso: música de fondo, saludo y resumen del día."
      />

      {status && (
        <div style={{ display: 'flex', gap: 12, marginBottom: 8 }}>
          <StatusCard
            label="Archivo de música"
            ok={status.music_file_present}
            okText="Encontrado"
            badText="Falta"
            detail={status.detail}
          />
        </div>
      )}

      <FieldGroup title="Escena">
        <Field label="Activar bienvenida por doble aplauso">
          <Toggle checked={cfg.enabled} onChange={(v) => set('enabled', v)} />
        </Field>
        <div />
        <Field label="Frase de saludo">
          <TextInput value={cfg.greeting_phrase} onChange={(v) => set('greeting_phrase', v)} />
        </Field>
        <Field label="Ruta del mp3" hint="Tuyo, con copyright — nunca se versiona en git.">
          <TextInput monospace value={cfg.music_path} onChange={(v) => set('music_path', v)} />
        </Field>
        <Field label="Contar noticias si no hay recordatorios">
          <Toggle checked={cfg.news_when_no_reminders} onChange={(v) => set('news_when_no_reminders', v)} />
        </Field>
      </FieldGroup>

      <FieldGroup title="Volumen y tiempos">
        <Field label="Volumen de la música">
          <Slider
            value={Math.round(cfg.music_volume * 100)}
            min={0}
            max={100}
            onChange={(v) => set('music_volume', v / 100)}
            format={(v) => `${v}%`}
          />
        </Field>
        <Field label="Volumen con alguien hablando" hint="Ducking mientras Jarvis o vos hablan.">
          <Slider
            value={Math.round(cfg.duck_volume * 100)}
            min={0}
            max={100}
            onChange={(v) => set('duck_volume', v / 100)}
            format={(v) => `${v}%`}
          />
        </Field>
        <Field label="Cooldown entre escenas">
          <Slider
            value={cfg.cooldown_secs}
            min={0}
            max={600}
            step={10}
            onChange={(v) => set('cooldown_secs', v)}
            format={(v) => `${v}s`}
          />
        </Field>
      </FieldGroup>

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={save} onDiscard={handleDiscard} />
    </div>
  );
}
