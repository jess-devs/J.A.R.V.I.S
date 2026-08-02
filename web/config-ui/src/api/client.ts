import type {
  AgentConfig,
  AudioConfig,
  BargeInConfig,
  LlmConfig,
  LlmStatus,
  McpServerConfig,
  PipelineConfig,
  SttConfig,
  TtsConfig,
  TtsStatus,
  WakeConfig,
  WelcomeConfig,
  WelcomeStatus,
  WorkersConfig,
} from "./types";

export class ApiError extends Error {}

async function extractError(res: Response): Promise<string> {
  try {
    const data = (await res.json()) as { error?: string };
    return data.error ?? `Error ${res.status}`;
  } catch {
    return `Error ${res.status}`;
  }
}

async function get<T>(path: string): Promise<T> {
  const res = await fetch(path);
  if (!res.ok) throw new ApiError(await extractError(res));
  return (await res.json()) as T;
}

async function put(path: string, body: unknown): Promise<void> {
  const res = await fetch(path, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new ApiError(await extractError(res));
}

export const api = {
  getLlm: () => get<LlmConfig>("/api/config/llm"),
  putLlm: (cfg: LlmConfig) => put("/api/config/llm", cfg),
  statusLlm: () => get<LlmStatus>("/api/status/llm"),

  getTts: () => get<TtsConfig>("/api/config/tts"),
  putTts: (cfg: TtsConfig) => put("/api/config/tts", cfg),
  statusTts: () => get<TtsStatus>("/api/status/tts"),

  getWorkers: () => get<WorkersConfig>("/api/config/workers"),
  putWorkers: (cfg: WorkersConfig) => put("/api/config/workers", cfg),

  getStt: () => get<SttConfig>("/api/config/stt"),
  putStt: (cfg: SttConfig) => put("/api/config/stt", cfg),

  getWake: () => get<WakeConfig>("/api/config/wake"),
  putWake: (cfg: WakeConfig) => put("/api/config/wake", cfg),

  getBargeIn: () => get<BargeInConfig>("/api/config/barge_in"),
  putBargeIn: (cfg: BargeInConfig) => put("/api/config/barge_in", cfg),

  getAudio: () => get<AudioConfig>("/api/config/audio"),
  putAudio: (cfg: AudioConfig) => put("/api/config/audio", cfg),

  getPipeline: () => get<PipelineConfig>("/api/config/pipeline"),
  putPipeline: (cfg: PipelineConfig) => put("/api/config/pipeline", cfg),

  getWelcome: () => get<WelcomeConfig>("/api/config/welcome"),
  putWelcome: (cfg: WelcomeConfig) => put("/api/config/welcome", cfg),
  statusWelcome: () => get<WelcomeStatus>("/api/status/welcome"),

  getMcp: () => get<McpServerConfig[]>("/api/config/mcp"),
  putMcp: (cfg: McpServerConfig[]) => put("/api/config/mcp", cfg),

  getAgent: () => get<AgentConfig>("/api/config/agent"),
  putAgent: (agent: AgentConfig, currentRiskCode?: string) =>
    put("/api/config/agent", {
      agent,
      current_risk_code: currentRiskCode ?? null,
    }),
};
