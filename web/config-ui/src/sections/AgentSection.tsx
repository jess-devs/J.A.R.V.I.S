import { useEffect, useRef, useState } from 'react';
import { ShieldAlert } from 'lucide-react';
import { api } from '../api/client';
import type { AgentConfig, ConfirmMode, OnUncertainPolicy } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { SaveBar } from '../components/SaveBar';
import { Modal } from '../components/Modal';
import { Field, FieldGroup } from '../components/form/Field';
import {
  KeyValueEditor,
  NumberInput,
  Select,
  Slider,
  StringListEditor,
  TextInput,
  Toggle,
} from '../components/form/Controls';
import styles from '../components/form/Controls.module.css';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';

const CONFIRM_MODE_OPTIONS: { value: ConfirmMode; label: string }[] = [
  { value: 'always', label: 'Siempre pedir confirmación de voz' },
  { value: 'free', label: 'Mano libre (sin preguntar)' },
];

const ON_UNCERTAIN_OPTIONS: { value: OnUncertainPolicy; label: string }[] = [
  { value: 'deny', label: 'Cancelar y pedir que lo repita (más seguro)' },
  { value: 'allow', label: 'Dejar pasar igual, solo con aviso en el log' },
];

export function AgentSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const riskCodeConfirmRef = useRef<string>('');
  const { value, original, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<AgentConfig>({
      load: api.getAgent,
      save: (agent) => api.putAgent(agent, riskCodeConfirmRef.current || undefined),
    });

  const [modalOpen, setModalOpen] = useState(false);
  const [modalInput, setModalInput] = useState('');
  const [voiceEnrolled, setVoiceEnrolled] = useState<boolean | null>(null);

  useEffect(() => {
    api
      .statusSpeakerVerification()
      .then((s) => setVoiceEnrolled(s.enrolled))
      .catch(() => setVoiceEnrolled(null));
  }, []);

  const doSave = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Sección Agente guardada.' });
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    } finally {
      riskCodeConfirmRef.current = '';
    }
  };

  const attemptSave = async () => {
    if (value && original && value.risk_code !== original.risk_code) {
      setModalInput('');
      setModalOpen(true);
      return;
    }
    await doSave();
  };

  const confirmRiskCodeChange = async () => {
    riskCodeConfirmRef.current = modalInput;
    setModalOpen(false);
    await doSave();
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const cfg = value;
  const set = <K extends keyof AgentConfig>(key: K, val: AgentConfig[K]) =>
    setValue((prev) => ({ ...prev, [key]: val }));

  const riskCodeChanged = original !== null && cfg.risk_code !== original.risk_code;

  return (
    <div>
      <PageHeader
        title="Agente"
        subtitle="Las herramientas que Jarvis puede ejecutar, y los tres niveles de riesgo que las gobiernan."
      />

      <FieldGroup title="General">
        <Field label="Capa agéntica activada" hint="Si no, chat puro sin herramientas.">
          <Toggle checked={cfg.enabled} onChange={(v) => set('enabled', v)} />
        </Field>
        <Field label="Modo de confirmación">
          <Select value={cfg.confirm_mode} onChange={(v) => set('confirm_mode', v as ConfirmMode)} options={CONFIRM_MODE_OPTIONS} />
        </Field>
        <Field label="Máximo de pasadas LLM → herramientas">
          <NumberInput value={cfg.max_iterations} min={1} max={20} onChange={(v) => set('max_iterations', v)} />
        </Field>
        <Field label="Timeout de herramienta">
          <NumberInput value={cfg.tool_timeout_secs} min={1} max={120} suffix="s" onChange={(v) => set('tool_timeout_secs', v)} />
        </Field>
        <Field label="Timeout de confirmación">
          <NumberInput value={cfg.confirm_timeout_secs} min={5} max={120} suffix="s" onChange={(v) => set('confirm_timeout_secs', v)} />
        </Field>
        <Field label="Truncado de resultado para el LLM">
          <NumberInput value={cfg.max_tool_result_chars} min={200} max={20000} step={100} onChange={(v) => set('max_tool_result_chars', v)} />
        </Field>
      </FieldGroup>

      <FieldGroup title="Código de aceptación" columns={1}>
        <Field
          label="risk_code"
          hint="La red de seguridad final para acciones de riesgo extremo — Jarvis lo pide hablado y nunca se lo pasa al LLM. Cambiarlo acá pide reingresar el código actual."
        >
          <div className={riskCodeChanged ? styles.riskCodeChanged : undefined}>
            <TextInput monospace value={cfg.risk_code} onChange={(v) => set('risk_code', v)} />
          </div>
          {riskCodeChanged && (
            <p className={styles.riskCodeNotice}>
              <ShieldAlert size={13} strokeWidth={2} />
              Cambiaste el código — al guardar te voy a pedir el actual para confirmar.
            </p>
          )}
        </Field>
        <Field label="Patrones de riesgo extremo adicionales" hint="Regex que se suman a los defaults.">
          <StringListEditor values={cfg.high_risk_patterns} onChange={(v) => set('high_risk_patterns', v)} placeholder="ej. rm -rf" />
        </Field>
      </FieldGroup>

      <FieldGroup title="Frases y listas" columns={2}>
        <Field label="Confirmación afirmativa">
          <StringListEditor values={cfg.confirm_yes} onChange={(v) => set('confirm_yes', v)} placeholder="ej. dale" />
        </Field>
        <Field label="Confirmación negativa">
          <StringListEditor values={cfg.confirm_no} onChange={(v) => set('confirm_no', v)} placeholder="ej. cancela" />
        </Field>
        <Field label="Frases de relleno mientras ejecuta">
          <StringListEditor values={cfg.filler_phrases} onChange={(v) => set('filler_phrases', v)} placeholder="ej. un momento" />
        </Field>
        <Field label="Herramientas deshabilitadas">
          <StringListEditor values={cfg.disabled_tools} onChange={(v) => set('disabled_tools', v)} placeholder="ej. run_powershell" />
        </Field>
      </FieldGroup>

      <FieldGroup title="Archivos">
        <Field label="Tope de resultados">
          <NumberInput value={cfg.files.max_results} min={1} max={200} onChange={(v) => set('files', { ...cfg.files, max_results: v })} />
        </Field>
        <Field label="Ruta a Everything CLI" hint="Vacío = recorrido acotado con walkdir.">
          <TextInput
            monospace
            placeholder="es.exe"
            value={cfg.files.everything_cli ?? ''}
            onChange={(v) => set('files', { ...cfg.files, everything_cli: v === '' ? null : v })}
          />
        </Field>
        <div style={{ gridColumn: '1 / -1' }}>
          <Field label="Carpetas de búsqueda">
            <StringListEditor
              values={cfg.files.search_roots}
              onChange={(v) => set('files', { ...cfg.files, search_roots: v })}
              placeholder="ej. C:\Users\vos\Documents"
            />
          </Field>
        </div>
      </FieldGroup>

      <FieldGroup title="Aplicaciones" columns={1}>
        <Field label="Alias hablado → ejecutable">
          <KeyValueEditor
            value={cfg.apps.aliases}
            onChange={(v) => set('apps', { ...cfg.apps, aliases: v })}
            keyPlaceholder="navegador"
            valuePlaceholder="brave"
          />
        </Field>
        <Field label="Carpetas extra de búsqueda de apps">
          <StringListEditor
            values={cfg.apps.extra_search_roots}
            onChange={(v) => set('apps', { ...cfg.apps, extra_search_roots: v })}
          />
        </Field>
      </FieldGroup>

      <FieldGroup title="Búsqueda web">
        <Field label="Caracteres de página truncados">
          <NumberInput value={cfg.web.max_page_chars} min={500} max={20000} step={100} onChange={(v) => set('web', { ...cfg.web, max_page_chars: v })} />
        </Field>
        <Field label="Tope de resultados de búsqueda">
          <NumberInput value={cfg.web.max_results} min={1} max={20} onChange={(v) => set('web', { ...cfg.web, max_results: v })} />
        </Field>
        <Field label="Permitir IPs privadas/loopback" hint="Protección anti-SSRF, desactivada por defecto.">
          <Toggle checked={cfg.web.allow_private_network} onChange={(v) => set('web', { ...cfg.web, allow_private_network: v })} />
        </Field>
        <div />
        <div style={{ gridColumn: '1 / -1' }}>
          <Field label="User-Agent">
            <TextInput monospace value={cfg.web.user_agent} onChange={(v) => set('web', { ...cfg.web, user_agent: v })} />
          </Field>
        </div>
      </FieldGroup>

      <FieldGroup title="Memoria y traducción">
        <Field label="Base de datos de memoria">
          <TextInput monospace value={cfg.memory.db_path} onChange={(v) => set('memory', { ...cfg.memory, db_path: v })} />
        </Field>
        <Field label="Memorias inyectadas por turno">
          <NumberInput value={cfg.memory.max_injected} min={0} max={50} onChange={(v) => set('memory', { ...cfg.memory, max_injected: v })} />
        </Field>
        <Field label="Idioma destino de traducción por defecto">
          <TextInput value={cfg.translate.default_target_lang} onChange={(v) => set('translate', { default_target_lang: v })} />
        </Field>
      </FieldGroup>

      <FieldGroup title="Recordatorios">
        <Field label="Base de datos de recordatorios">
          <TextInput monospace value={cfg.reminders.db_path} onChange={(v) => set('reminders', { ...cfg.reminders, db_path: v })} />
        </Field>
        <Field label="Intervalo de revisión">
          <NumberInput value={cfg.reminders.poll_interval_secs} min={5} max={300} suffix="s" onChange={(v) => set('reminders', { ...cfg.reminders, poll_interval_secs: v })} />
        </Field>
        <Field label="Máximo de recordatorios activos">
          <NumberInput value={cfg.reminders.max_active} min={1} max={500} onChange={(v) => set('reminders', { ...cfg.reminders, max_active: v })} />
        </Field>
      </FieldGroup>

      <FieldGroup title="Tools personalizadas">
        <Field label="Base de datos de tools">
          <TextInput monospace value={cfg.scripted_tools.db_path} onChange={(v) => set('scripted_tools', { ...cfg.scripted_tools, db_path: v })} />
        </Field>
        <Field label="Máximo de tools simultáneas">
          <NumberInput value={cfg.scripted_tools.max_tools} min={1} max={200} onChange={(v) => set('scripted_tools', { ...cfg.scripted_tools, max_tools: v })} />
        </Field>
        <Field label="Timeout de recetas HTTP">
          <NumberInput value={cfg.scripted_tools.http_timeout_secs} min={1} max={120} suffix="s" onChange={(v) => set('scripted_tools', { ...cfg.scripted_tools, http_timeout_secs: v })} />
        </Field>
        <Field label="Permitir IPs privadas/loopback">
          <Toggle
            checked={cfg.scripted_tools.allow_private_network}
            onChange={(v) => set('scripted_tools', { ...cfg.scripted_tools, allow_private_network: v })}
          />
        </Field>
        <div style={{ gridColumn: '1 / -1' }}>
          <Field label="Hosts permitidos" hint="Vacío = sin restricción.">
            <StringListEditor
              values={cfg.scripted_tools.allowed_hosts}
              onChange={(v) => set('scripted_tools', { ...cfg.scripted_tools, allowed_hosts: v })}
              placeholder="ej. api.miservicio.com"
            />
          </Field>
        </div>
      </FieldGroup>

      <FieldGroup title="Auditoría y verificación de hablante">
        <Field label="Log de auditoría activado">
          <Toggle checked={cfg.audit.enabled} onChange={(v) => set('audit', { ...cfg.audit, enabled: v })} />
        </Field>
        <Field label="Ruta del log">
          <TextInput monospace value={cfg.audit.path} onChange={(v) => set('audit', { ...cfg.audit, path: v })} />
        </Field>
        <Field
          label="Verificación de hablante (modo sombra)"
          hint="Calcula y loguea la similitud de cada frase contra tu voz enrolada, sin bloquear nada todavía."
        >
          <Toggle
            checked={cfg.speaker_verification.enabled}
            onChange={(v) => set('speaker_verification', { ...cfg.speaker_verification, enabled: v })}
          />
        </Field>
      </FieldGroup>

      <FieldGroup title="Gating de confirmaciones por voz">
        <Field
          label="Exigir tu voz para confirmar acciones de riesgo"
          hint={
            voiceEnrolled === false
              ? 'Enrolá tu voz primero desde la sección "Micrófono" del sidebar.'
              : 'Si la voz no coincide (o no se pudo verificar a tiempo), la acción se cancela y te lo dice.'
          }
        >
          <Toggle
            checked={cfg.speaker_verification.gate_confirmations}
            disabled={voiceEnrolled === false}
            onChange={(v) => set('speaker_verification', { ...cfg.speaker_verification, gate_confirmations: v })}
          />
        </Field>
        <div />
        <Field
          label="Umbral de similitud"
          hint="Punto de partida, no una recomendación de seguridad — ajustalo mirando tus propios valores logueados en modo sombra."
        >
          <Slider
            value={Math.round(cfg.speaker_verification.similarity_threshold * 100)}
            min={0}
            max={100}
            onChange={(v) => set('speaker_verification', { ...cfg.speaker_verification, similarity_threshold: v / 100 })}
            format={(v) => `${v}%`}
          />
        </Field>
        <Field label="Tiempo de espera de la verificación">
          <NumberInput
            value={cfg.speaker_verification.gate_wait_ms}
            min={100}
            max={5000}
            step={100}
            suffix="ms"
            onChange={(v) => set('speaker_verification', { ...cfg.speaker_verification, gate_wait_ms: v })}
          />
        </Field>
        <Field label="Si no se pudo verificar a tiempo">
          <Select
            value={cfg.speaker_verification.on_uncertain}
            onChange={(v) => set('speaker_verification', { ...cfg.speaker_verification, on_uncertain: v as OnUncertainPolicy })}
            options={ON_UNCERTAIN_OPTIONS}
          />
        </Field>
      </FieldGroup>

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={attemptSave} onDiscard={handleDiscard} />

      {modalOpen && (
        <Modal title="Confirmá el código actual" onClose={() => setModalOpen(false)}>
          <p className={styles.modalBody}>
            Cambiaste <code>risk_code</code>. Ingresá el código de aceptación actual (el que sigue vigente hasta que
            confirmes) para autorizar el cambio.
          </p>
          <TextInput monospace value={modalInput} onChange={setModalInput} placeholder="código actual" />
          <div className={styles.modalActions}>
            <button type="button" className={styles.secondaryButton} onClick={() => setModalOpen(false)}>
              Cancelar
            </button>
            <button type="button" className={styles.primaryButton} onClick={confirmRiskCodeChange} disabled={!modalInput}>
              Confirmar y guardar
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
