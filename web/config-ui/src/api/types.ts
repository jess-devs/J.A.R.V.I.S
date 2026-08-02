// Espejo de src/config.rs (secciones llm y tts) — mantener sincronizado a mano,
// no hay generación automática de tipos en este piloto.

export type LlmProvider =
  "ollama" | "anthropic" | "openai" | "deepseek" | "lmstudio";

export interface OllamaConfig {
  base_url: string;
  model: string;
  think: boolean | null;
  auto_serve: boolean;
  vision_model: string | null;
}

export interface AnthropicConfig {
  model: string;
  api_key_env: string;
}

export interface OpenAiConfig {
  model: string;
  api_key_env: string;
}

export interface DeepSeekConfig {
  model: string;
  api_key_env: string;
}

export interface LmStudioConfig {
  base_url: string;
  model: string;
  api_key_env: string | null;
}

export interface LlmConfig {
  provider: LlmProvider;
  ollama: OllamaConfig;
  anthropic: AnthropicConfig;
  openai: OpenAiConfig;
  deepseek: DeepSeekConfig;
  lmstudio: LmStudioConfig;
  system_prompt: string;
  max_history_messages: number;
  request_timeout_secs: number;
}

export interface LlmStatus {
  provider: LlmProvider;
  reachable: boolean | null;
  api_key_present: boolean | null;
  detail: string;
}

export type TtsProvider = "piper" | "elevenlabs" | "cartesia";

export interface PiperConfig {
  voice_path: string;
  config_path: string;
  use_cuda: boolean;
  length_scale: number | null;
  noise_w_scale: number | null;
}

export interface ElevenLabsConfig {
  voice_id: string;
  model_id: string;
  output_format: string;
  api_key_env: string;
}

export type CartesiaTransport = "rest" | "websocket";

export interface CartesiaOutputFormat {
  container: string;
  encoding: string;
  sample_rate: number;
}

export interface CartesiaConfig {
  model_id: string;
  voice_id: string;
  language: string | null;
  output_format: CartesiaOutputFormat;
  api_key_env: string;
  cartesia_version: string;
  transport: CartesiaTransport;
}

export interface TtsConfig {
  provider: TtsProvider;
  piper: PiperConfig;
  elevenlabs: ElevenLabsConfig;
  cartesia: CartesiaConfig;
  synth_timeout_secs: number;
}

export interface TtsStatus {
  provider: TtsProvider;
  voice_files_present: boolean | null;
  api_key_present: boolean | null;
  detail: string;
}

// ---------------------------------------------------------------------
// workers
// ---------------------------------------------------------------------

export interface WorkersConfig {
  python_executable: string;
  stt_script: string;
  tts_script: string;
  stt_init_timeout_secs: number;
  tts_init_timeout_secs: number;
  shutdown_timeout_secs: number;
  restart_on_crash: boolean;
  max_restarts: number;
}

// ---------------------------------------------------------------------
// stt
// ---------------------------------------------------------------------

export type SttEngine = "native" | "realtimestt";

export interface VadConfig {
  threshold: number;
  neg_threshold: number;
  pre_roll_ms: number;
  min_speech_ms: number;
  silence_long_ms: number;
  silence_short_ms: number;
  long_utterance_ms: number;
  energy_floor_dbfs: number | null;
  calibration_secs: number;
}

export interface SttFiltersConfig {
  max_no_speech_prob: number;
  min_avg_logprob: number;
  max_compression_ratio: number;
}

export interface ClapConfig {
  min_peak_dbfs: number;
  min_rise_db: number;
  decay_ms: number;
  max_vad_prob: number;
  min_zcr: number;
  double_min_gap_ms: number;
  double_max_gap_ms: number;
  refractory_ms: number;
}

export interface SttConfig {
  engine: SttEngine;
  vad: VadConfig;
  filters: SttFiltersConfig;
  clap: ClapConfig;
  language: string;
  device: string;
  whisper_model: string;
  compute_type: string;
  input_device_index: number | null;
  beam_size: number | null;
  cpu_threads: number | null;
  initial_prompt: string;
  recalibrate: boolean;
  silero_sensitivity: number;
  webrtc_sensitivity: number;
  post_speech_silence_duration: number;
  min_length_of_recording: number;
  min_gap_between_recordings: number;
  silero_deactivity_detection: boolean;
  stuck_state_timeout_secs: number;
}

// ---------------------------------------------------------------------
// wake
// ---------------------------------------------------------------------

export interface WakeConfig {
  enabled: boolean;
  words: string[];
  attention_window_secs: number;
  window_min_words: number;
  ignore_phrases: string[];
  ambient_context: boolean;
  ambient_context_max: number;
  ambient_context_ttl_secs: number;
}

// ---------------------------------------------------------------------
// barge_in
// ---------------------------------------------------------------------

export type BargeInMode = "wake_word" | "any_voice";

export interface EchoGuardConfig {
  enabled: boolean;
  similarity_threshold: number;
  vad_threshold_while_speaking: number;
  recent_tts_window_secs: number;
}

export interface BargeInConfig {
  enabled: boolean;
  mode: BargeInMode;
  min_speech_ms: number;
  echo_guard: EchoGuardConfig;
  relevance_timeout_secs: number;
}

// ---------------------------------------------------------------------
// audio
// ---------------------------------------------------------------------

export interface AudioConfig {
  output_device: string | null;
  volume: number;
  drain_timeout_secs: number;
}

// ---------------------------------------------------------------------
// pipeline
// ---------------------------------------------------------------------

export interface PipelineConfig {
  max_phrase_chars: number;
  min_phrase_chars: number;
}

// ---------------------------------------------------------------------
// agent
// ---------------------------------------------------------------------

export type ConfirmMode = "always" | "free";

export interface FilesToolConfig {
  search_roots: string[];
  max_results: number;
  everything_cli: string | null;
}

export interface AppsConfig {
  aliases: Record<string, string>;
  extra_search_roots: string[];
}

export interface WebToolConfig {
  max_page_chars: number;
  max_results: number;
  user_agent: string;
  allow_private_network: boolean;
}

export interface MemoryConfig {
  db_path: string;
  max_injected: number;
}

export interface TranslateConfig {
  default_target_lang: string;
}

export interface RemindersConfig {
  db_path: string;
  poll_interval_secs: number;
  max_active: number;
}

export interface ScriptedToolsConfig {
  db_path: string;
  max_tools: number;
  http_timeout_secs: number;
  allowed_hosts: string[];
  allow_private_network: boolean;
}

export interface AuditConfig {
  enabled: boolean;
  path: string;
}

export interface SpeakerVerificationConfig {
  enabled: boolean;
}

export interface AgentConfig {
  enabled: boolean;
  max_iterations: number;
  tool_timeout_secs: number;
  confirm_timeout_secs: number;
  confirm_mode: ConfirmMode;
  max_tool_result_chars: number;
  filler_phrases: string[];
  disabled_tools: string[];
  confirm_yes: string[];
  confirm_no: string[];
  risk_code: string;
  high_risk_patterns: string[];
  files: FilesToolConfig;
  apps: AppsConfig;
  web: WebToolConfig;
  memory: MemoryConfig;
  translate: TranslateConfig;
  reminders: RemindersConfig;
  scripted_tools: ScriptedToolsConfig;
  audit: AuditConfig;
  speaker_verification: SpeakerVerificationConfig;
}

// ---------------------------------------------------------------------
// mcp
// ---------------------------------------------------------------------

export interface McpServerConfig {
  name: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  trusted_tools: string[];
}

// ---------------------------------------------------------------------
// welcome
// ---------------------------------------------------------------------

export interface WelcomeConfig {
  enabled: boolean;
  music_path: string;
  greeting_phrase: string;
  music_volume: number;
  duck_volume: number;
  cooldown_secs: number;
  news_when_no_reminders: boolean;
}

export interface WelcomeStatus {
  music_file_present: boolean;
  detail: string;
}
