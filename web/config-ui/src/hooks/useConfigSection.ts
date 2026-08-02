import { useCallback, useEffect, useState } from 'react';

interface UseConfigSectionOptions<T> {
  load: () => Promise<T>;
  save: (value: T) => Promise<void>;
}

interface UseConfigSectionResult<T> {
  value: T | null;
  original: T | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  dirty: boolean;
  savedOnce: boolean;
  setValue: (updater: T | ((prev: T) => T)) => void;
  handleSave: () => Promise<void>;
  handleDiscard: () => void;
}

/** Carga, edita y guarda una sección de config.yaml (ej. llm o tts). */
export function useConfigSection<T>({ load, save }: UseConfigSectionOptions<T>): UseConfigSectionResult<T> {
  const [original, setOriginal] = useState<T | null>(null);
  const [value, setValueState] = useState<T | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedOnce, setSavedOnce] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    load()
      .then((data) => {
        if (cancelled) return;
        setOriginal(data);
        setValueState(data);
        setError(null);
      })
      .catch((e: Error) => !cancelled && setError(e.message))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const setValue = useCallback((updater: T | ((prev: T) => T)) => {
    setValueState((prev) => {
      if (prev === null) return prev;
      return typeof updater === 'function' ? (updater as (prev: T) => T)(prev) : updater;
    });
  }, []);

  const dirty = original !== null && value !== null && JSON.stringify(original) !== JSON.stringify(value);

  const handleSave = useCallback(async () => {
    if (value === null) return;
    setSaving(true);
    setError(null);
    try {
      await save(value);
      setOriginal(value);
      setSavedOnce(true);
    } catch (e) {
      setError((e as Error).message);
      throw e;
    } finally {
      setSaving(false);
    }
  }, [value, save]);

  const handleDiscard = useCallback(() => {
    setValueState(original);
  }, [original]);

  return { value, original, loading, saving, error, dirty, savedOnce, setValue, handleSave, handleDiscard };
}
