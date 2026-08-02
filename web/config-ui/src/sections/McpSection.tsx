import { Plug, Plus, Trash2 } from 'lucide-react';
import { api } from '../api/client';
import type { McpServerConfig } from '../api/types';
import { PageHeader } from '../components/PageHeader';
import { SaveBar } from '../components/SaveBar';
import { Field } from '../components/form/Field';
import { KeyValueEditor, StringListEditor, TextInput } from '../components/form/Controls';
import { useConfigSection } from '../hooks/useConfigSection';
import type { ToastMessage } from '../components/Toast';
import styles from './McpSection.module.css';

const EMPTY_SERVER: McpServerConfig = {
  name: '',
  command: '',
  args: [],
  env: {},
  trusted_tools: [],
};

export function McpSection({ onToast }: { onToast: (toast: Omit<ToastMessage, 'id'>) => void }) {
  const { value, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard } =
    useConfigSection<McpServerConfig[]>({ load: api.getMcp, save: api.putMcp });

  const save = async () => {
    try {
      await handleSave();
      onToast({ tone: 'success', text: 'Sección Servidores MCP guardada.' });
    } catch (e) {
      onToast({ tone: 'error', text: (e as Error).message });
    }
  };

  if (loading) return <p>Cargando…</p>;
  if (error && !value) return <p role="alert">No se pudo cargar: {error}</p>;
  if (!value) return null;

  const servers = value;
  const updateServer = (index: number, patch: Partial<McpServerConfig>) => {
    setValue((prev) => prev.map((s, i) => (i === index ? { ...s, ...patch } : s)));
  };
  const removeServer = (index: number) => {
    setValue((prev) => prev.filter((_, i) => i !== index));
  };
  const addServer = () => {
    setValue((prev) => [...prev, { ...EMPTY_SERVER }]);
  };

  return (
    <div>
      <PageHeader
        title="Servidores MCP"
        subtitle="Servidores externos (Model Context Protocol) a los que Jarvis se conecta como cliente. Complementa create_tool, no lo reemplaza."
      />

      {servers.length === 0 && (
        <div className={styles.empty}>
          <Plug size={20} strokeWidth={1.75} />
          <p>Sin servidores configurados todavía.</p>
        </div>
      )}

      <div className={styles.list}>
        {servers.map((server, i) => (
          <div className={styles.card} key={i}>
            <div className={styles.cardHeader}>
              <span className={styles.cardTitle}>{server.name || `Servidor sin nombre #${i + 1}`}</span>
              <button
                type="button"
                className={styles.removeButton}
                aria-label={`Quitar ${server.name || 'servidor'}`}
                onClick={() => removeServer(i)}
              >
                <Trash2 size={14} strokeWidth={2} />
                Quitar
              </button>
            </div>

            <div className={styles.cardGrid}>
              <Field label="Nombre">
                <TextInput value={server.name} onChange={(v) => updateServer(i, { name: v })} placeholder="ej. everything" />
              </Field>
              <Field label="Comando" hint="Proceso hijo, transporte stdio.">
                <TextInput monospace value={server.command} onChange={(v) => updateServer(i, { command: v })} placeholder="ej. npx" />
              </Field>
            </div>

            <Field label="Argumentos">
              <StringListEditor
                values={server.args}
                onChange={(v) => updateServer(i, { args: v })}
                placeholder='ej. -y @modelcontextprotocol/server-everything'
              />
            </Field>

            <Field label="Variables de entorno">
              <KeyValueEditor value={server.env} onChange={(v) => updateServer(i, { env: v })} />
            </Field>

            <Field
              label="Tools de confianza"
              hint="Se ejecutan sin pedir confirmación (default: Confirm para todas; nunca Code)."
            >
              <StringListEditor
                values={server.trusted_tools}
                onChange={(v) => updateServer(i, { trusted_tools: v })}
                placeholder="ej. echo"
              />
            </Field>
          </div>
        ))}
      </div>

      <button type="button" className={styles.addButton} onClick={addServer}>
        <Plus size={15} strokeWidth={2} />
        Agregar servidor
      </button>

      <SaveBar dirty={dirty} saving={saving} needsRestart={savedOnce} onSave={save} onDiscard={handleDiscard} />
    </div>
  );
}
