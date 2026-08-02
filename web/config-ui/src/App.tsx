import { useCallback, useEffect, useState } from 'react';
import { api } from './api/client';
import { Sidebar } from './components/Sidebar';
import { WorkersSection } from './sections/WorkersSection';
import { OnboardingSection } from './sections/OnboardingSection';
import { SttSection } from './sections/SttSection';
import { WakeSection } from './sections/WakeSection';
import { BargeInSection } from './sections/BargeInSection';
import { LlmSection } from './sections/LlmSection';
import { TtsSection } from './sections/TtsSection';
import { AudioSection } from './sections/AudioSection';
import { PipelineSection } from './sections/PipelineSection';
import { AgentSection } from './sections/AgentSection';
import { McpSection } from './sections/McpSection';
import { WelcomeSection } from './sections/WelcomeSection';
import { OnboardingWizard } from './onboarding/OnboardingWizard';
import { ToastStack, type ToastMessage } from './components/Toast';
import styles from './App.module.css';

let toastSeq = 0;

export default function App() {
  const [activeId, setActiveId] = useState('llm');
  const [toasts, setToasts] = useState<ToastMessage[]>([]);
  // `null` = todavía no se sabe (cargando `GET /api/config/onboarding`) —
  // se trata igual que "completado" para no mostrar el wizard de golpe si
  // la red tarda; `false` es el único caso que dispara el wizard.
  const [onboardingCompleted, setOnboardingCompleted] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getOnboarding()
      .then((cfg) => !cancelled && setOnboardingCompleted(cfg.completed))
      .catch(() => !cancelled && setOnboardingCompleted(true));
    return () => {
      cancelled = true;
    };
  }, []);

  const pushToast = useCallback((toast: Omit<ToastMessage, 'id'>) => {
    const id = ++toastSeq;
    setToasts((prev) => [...prev, { ...toast, id }]);
  }, []);

  const dismissToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  if (onboardingCompleted === false) {
    return <OnboardingWizard onDone={() => setOnboardingCompleted(true)} />;
  }

  return (
    <div className={styles.page}>
      <div className={styles.shell}>
        <Sidebar activeId={activeId} onSelect={setActiveId} />
        <main className={styles.content}>
          <div className={styles.contentInner}>
            {activeId === 'onboarding' && <OnboardingSection onToast={pushToast} />}
            {activeId === 'workers' && <WorkersSection onToast={pushToast} />}
            {activeId === 'stt' && <SttSection onToast={pushToast} />}
            {activeId === 'wake' && <WakeSection onToast={pushToast} />}
            {activeId === 'barge_in' && <BargeInSection onToast={pushToast} />}
            {activeId === 'llm' && <LlmSection onToast={pushToast} />}
            {activeId === 'tts' && <TtsSection onToast={pushToast} />}
            {activeId === 'audio' && <AudioSection onToast={pushToast} />}
            {activeId === 'pipeline' && <PipelineSection onToast={pushToast} />}
            {activeId === 'agent' && <AgentSection onToast={pushToast} />}
            {activeId === 'mcp' && <McpSection onToast={pushToast} />}
            {activeId === 'welcome' && <WelcomeSection onToast={pushToast} />}
          </div>
        </main>
      </div>
      <ToastStack toasts={toasts} onDismiss={dismissToast} />
    </div>
  );
}
