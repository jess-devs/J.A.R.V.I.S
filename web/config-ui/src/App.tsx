import { useCallback, useState } from 'react';
import { Sidebar } from './components/Sidebar';
import { WorkersSection } from './sections/WorkersSection';
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
import { ToastStack, type ToastMessage } from './components/Toast';
import styles from './App.module.css';

let toastSeq = 0;

export default function App() {
  const [activeId, setActiveId] = useState('llm');
  const [toasts, setToasts] = useState<ToastMessage[]>([]);

  const pushToast = useCallback((toast: Omit<ToastMessage, 'id'>) => {
    const id = ++toastSeq;
    setToasts((prev) => [...prev, { ...toast, id }]);
  }, []);

  const dismissToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  return (
    <div className={styles.page}>
      <div className={styles.shell}>
        <Sidebar activeId={activeId} onSelect={setActiveId} />
        <main className={styles.content}>
          <div className={styles.contentInner}>
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
