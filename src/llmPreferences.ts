import { invoke } from "@tauri-apps/api/core";

export type LlmProvider = "local" | "inception";

export type LlmPreferences = {
  defaultProvider: LlmProvider;
  alwaysObeyGlobal: boolean;
};

type LlamaPreferenceConfig = {
  default_provider: LlmProvider;
  always_obey_global_llm: boolean;
};

export async function loadLlmPreferences(): Promise<LlmPreferences> {
  const config = await invoke<LlamaPreferenceConfig>("get_llama_config");
  return {
    defaultProvider: config.default_provider,
    alwaysObeyGlobal: config.always_obey_global_llm,
  };
}
