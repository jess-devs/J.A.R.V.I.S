import type { LucideIcon } from "lucide-react";
import {
  AudioLines,
  Bot,
  BrainCircuit,
  Cpu,
  Mic,
  Plug,
  Sparkles,
  SlidersHorizontal,
  Volume2,
  Waves,
  Zap,
} from "lucide-react";

export interface GroupDef {
  id: string;
  label: string;
}

export interface SectionDef {
  id: string;
  label: string;
  icon: LucideIcon;
  enabled: boolean;
  group: string;
}

// Los 4 grupos funcionales del sidebar, en el orden en que se muestran.
export const GROUPS: GroupDef[] = [
  { id: "voice_in", label: "Entrada de voz" },
  { id: "voice_out", label: "Salida de voz" },
  { id: "intelligence", label: "Inteligencia y acciones" },
  { id: "system", label: "Sistema" },
];

// Las 11 secciones de config.yaml, agrupadas por función.
export const SECTIONS: SectionDef[] = [
  { id: "stt", label: "Voz (STT)", icon: Mic, enabled: true, group: "voice_in" },
  { id: "wake", label: "Palabra de activación", icon: Zap, enabled: true, group: "voice_in" },
  { id: "barge_in", label: "Interrupción", icon: Waves, enabled: true, group: "voice_in" },

  { id: "tts", label: "Síntesis de voz", icon: AudioLines, enabled: true, group: "voice_out" },
  { id: "audio", label: "Audio", icon: Volume2, enabled: true, group: "voice_out" },
  { id: "pipeline", label: "Pipeline", icon: SlidersHorizontal, enabled: true, group: "voice_out" },

  { id: "llm", label: "Modelo de lenguaje", icon: BrainCircuit, enabled: true, group: "intelligence" },
  { id: "agent", label: "Agente", icon: Bot, enabled: true, group: "intelligence" },
  { id: "mcp", label: "Servidores MCP", icon: Plug, enabled: true, group: "intelligence" },

  { id: "workers", label: "Workers", icon: Cpu, enabled: true, group: "system" },
  { id: "welcome", label: "Bienvenida", icon: Sparkles, enabled: true, group: "system" },
];
