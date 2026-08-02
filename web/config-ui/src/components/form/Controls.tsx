import { useState } from 'react';
import type { ChangeEvent, KeyboardEvent, ReactNode } from 'react';
import { CircleX, Plus, X } from 'lucide-react';
import styles from './Controls.module.css';

interface TextInputProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  monospace?: boolean;
  type?: 'text' | 'password';
}

export function TextInput({ value, onChange, placeholder, monospace, type = 'text' }: TextInputProps) {
  return (
    <input
      type={type}
      className={[styles.input, monospace ? styles.mono : ''].join(' ')}
      value={value}
      placeholder={placeholder}
      onChange={(e: ChangeEvent<HTMLInputElement>) => onChange(e.target.value)}
    />
  );
}

export function TextArea({
  value,
  onChange,
  rows = 6,
}: {
  value: string;
  onChange: (value: string) => void;
  rows?: number;
}) {
  return (
    <textarea
      className={styles.textarea}
      value={value}
      rows={rows}
      onChange={(e: ChangeEvent<HTMLTextAreaElement>) => onChange(e.target.value)}
    />
  );
}

interface SelectOption {
  value: string;
  label: string;
}

interface SelectProps {
  value: string;
  onChange: (value: string) => void;
  options: SelectOption[];
}

export function Select({ value, onChange, options }: SelectProps) {
  return (
    <div className={styles.selectWrap}>
      <select
        className={styles.select}
        value={value}
        onChange={(e: ChangeEvent<HTMLSelectElement>) => onChange(e.target.value)}
      >
        {options.map((opt) => (
          <option key={opt.value} value={opt.value}>
            {opt.label}
          </option>
        ))}
      </select>
      <svg className={styles.selectChevron} width="10" height="6" viewBox="0 0 10 6" fill="none" aria-hidden="true">
        <path d="M1 1L5 5L9 1" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    </div>
  );
}

export function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      className={[styles.toggle, checked ? styles.toggleOn : ''].join(' ')}
      onClick={() => onChange(!checked)}
    >
      <span className={styles.toggleThumb} />
      {label && <span className={styles.toggleLabel}>{label}</span>}
    </button>
  );
}

interface SliderProps {
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (value: number) => void;
  format?: (value: number) => string;
  leftLabel?: string;
  rightLabel?: string;
}

export function Slider({ value, min, max, step = 1, onChange, format, leftLabel, rightLabel }: SliderProps) {
  return (
    <div className={styles.sliderWrap}>
      <div className={styles.sliderTrackRow}>
        <input
          type="range"
          className={styles.slider}
          min={min}
          max={max}
          step={step}
          value={value}
          onChange={(e: ChangeEvent<HTMLInputElement>) => onChange(Number(e.target.value))}
        />
        <span className={styles.sliderValue}>{format ? format(value) : value}</span>
      </div>
      {(leftLabel || rightLabel) && (
        <div className={styles.sliderScale}>
          <span>{leftLabel}</span>
          <span>{rightLabel}</span>
        </div>
      )}
    </div>
  );
}

export function OptionalSlider({
  value,
  defaultValue,
  min,
  max,
  step = 0.01,
  onChange,
  format,
}: {
  value: number | null;
  defaultValue: number;
  min: number;
  max: number;
  step?: number;
  onChange: (value: number | null) => void;
  format?: (value: number) => string;
}) {
  const enabled = value !== null;
  return (
    <div className={styles.optionalRow}>
      <Toggle checked={enabled} onChange={(on) => onChange(on ? defaultValue : null)} />
      <div className={enabled ? undefined : styles.sliderDisabled}>
        <Slider value={value ?? defaultValue} min={min} max={max} step={step} onChange={onChange} format={format} />
      </div>
    </div>
  );
}

/** Como OptionalSlider pero para valores sin rango natural (ej. índice de dispositivo). */
export function OptionalNumberInput({
  value,
  defaultValue,
  min,
  max,
  onChange,
  suffix,
}: {
  value: number | null;
  defaultValue: number;
  min?: number;
  max?: number;
  onChange: (value: number | null) => void;
  suffix?: string;
}) {
  const enabled = value !== null;
  return (
    <div className={styles.optionalRow}>
      <Toggle checked={enabled} onChange={(on) => onChange(on ? defaultValue : null)} />
      <div className={enabled ? undefined : styles.sliderDisabled}>
        <NumberInput value={value ?? defaultValue} min={min} max={max} onChange={onChange} suffix={suffix} />
      </div>
    </div>
  );
}

export function Pill({ tone, children }: { tone: 'positive' | 'danger' | 'neutral'; children: ReactNode }) {
  return <span className={[styles.pill, styles[`pill_${tone}`]].join(' ')}>{children}</span>;
}

interface NumberInputProps {
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  suffix?: string;
}

export function NumberInput({ value, onChange, min, max, step = 1, suffix }: NumberInputProps) {
  return (
    <div className={styles.numberWrap}>
      <input
        type="number"
        className={[styles.input, styles.number].join(' ')}
        value={value}
        min={min}
        max={max}
        step={step}
        onChange={(e: ChangeEvent<HTMLInputElement>) => {
          const n = e.target.valueAsNumber;
          if (!Number.isNaN(n)) onChange(n);
        }}
      />
      {suffix && <span className={styles.numberSuffix}>{suffix}</span>}
    </div>
  );
}

/** Lista editable de strings (chips), para Vec&lt;String&gt; como `words` o `ignore_phrases`. */
export function StringListEditor({
  values,
  onChange,
  placeholder,
}: {
  values: string[];
  onChange: (values: string[]) => void;
  placeholder?: string;
}) {
  const [draft, setDraft] = useState('');

  const commit = () => {
    const trimmed = draft.trim();
    if (trimmed && !values.includes(trimmed)) onChange([...values, trimmed]);
    setDraft('');
  };

  return (
    <div className={styles.chipEditor}>
      {values.length > 0 && (
        <ul className={styles.chipList}>
          {values.map((v, i) => (
            <li key={`${v}-${i}`} className={styles.chip}>
              <span className={styles.chipText}>{v}</span>
              <button
                type="button"
                className={styles.chipRemove}
                aria-label={`Quitar "${v}"`}
                onClick={() => onChange(values.filter((_, idx) => idx !== i))}
              >
                <CircleX size={14} strokeWidth={1.75} />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className={styles.chipAddRow}>
        <input
          type="text"
          className={styles.chipInput}
          value={draft}
          placeholder={placeholder ?? 'Agregar y Enter'}
          onChange={(e: ChangeEvent<HTMLInputElement>) => setDraft(e.target.value)}
          onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              commit();
            }
          }}
        />
        <button type="button" className={styles.chipAddButton} onClick={commit} aria-label="Agregar">
          <Plus size={14} strokeWidth={2} />
        </button>
      </div>
    </div>
  );
}

/** Editor de pares clave-valor, para HashMap&lt;String,String&gt; como `apps.aliases`. */
export function KeyValueEditor({
  value,
  onChange,
  keyPlaceholder = 'clave',
  valuePlaceholder = 'valor',
}: {
  value: Record<string, string>;
  onChange: (value: Record<string, string>) => void;
  keyPlaceholder?: string;
  valuePlaceholder?: string;
}) {
  const entries = Object.entries(value);

  const updateRow = (index: number, key: string, val: string) => {
    const next = [...entries];
    next[index] = [key, val];
    onChange(Object.fromEntries(next));
  };

  const removeRow = (index: number) => {
    onChange(Object.fromEntries(entries.filter((_, i) => i !== index)));
  };

  const addRow = () => {
    let key = 'nueva_clave';
    let n = 2;
    while (key in value) {
      key = `nueva_clave_${n}`;
      n += 1;
    }
    onChange({ ...value, [key]: '' });
  };

  return (
    <div className={styles.kvEditor}>
      {entries.map(([k, v], i) => (
        <div className={styles.kvRow} key={i}>
          <input
            type="text"
            className={[styles.input, styles.kvKey].join(' ')}
            value={k}
            placeholder={keyPlaceholder}
            onChange={(e: ChangeEvent<HTMLInputElement>) => updateRow(i, e.target.value, v)}
          />
          <input
            type="text"
            className={[styles.input, styles.kvValue].join(' ')}
            value={v}
            placeholder={valuePlaceholder}
            onChange={(e: ChangeEvent<HTMLInputElement>) => updateRow(i, k, e.target.value)}
          />
          <button type="button" className={styles.kvRemove} aria-label={`Quitar ${k}`} onClick={() => removeRow(i)}>
            <X size={14} strokeWidth={2} />
          </button>
        </div>
      ))}
      <button type="button" className={styles.kvAddButton} onClick={addRow}>
        <Plus size={14} strokeWidth={2} />
        Agregar
      </button>
    </div>
  );
}
