// Mirrors the serde output of le_core::compare::Verdict and the backend's Card / settings.

export type VerdictLabel = "UPGRADE" | "SIDEGRADE" | "WORSE" | "REVIEW" | "UNKNOWN";

export interface AffixScore {
  name: string;
  tier: number;
  value: number | null;
  is_percent: boolean;
  stat: string | null;
  contribution: number;
  note: string;
}

export interface ItemScore {
  total: number;
  offense: number;
  defense: number;
  affixes: AffixScore[];
  unrecognised: string[];
  notes: string[];
}

export interface Verdict {
  label: VerdictLabel;
  slot: string | null;
  candidate_name: string;
  candidate_score: number;
  equipped_name: string | null;
  equipped_score: number;
  delta: number;
  reasons: string[];
  warnings: string[];
  candidate: ItemScore;
  equipped: ItemScore | null;
}

export interface AiInfo {
  model: string;
  summary: string;
  confidence: string;
  cached: boolean;
}

export interface Card {
  verdict: Verdict;
  /** logical pixels, relative to the overlay window */
  x: number;
  y: number;
  seconds: number;
  /** item rarity + type, e.g. "Rare Boots" */
  subtitle: string;
  equipped_from_tooltip: boolean;
  ai: AiInfo | null;
}

export interface Status {
  text: string;
  kind: "info" | "error";
}

// ---- Builds & AI window ----

export type Provider = "anthropic" | "openai" | "kimi" | "deepseek" | "openai-compatible";

export interface ModelConfig {
  provider: Provider | string;
  model: string;
  api_key: string;
  base_url: string;
  vision: boolean;
  effort: "low" | "medium" | "high" | string;
}

export interface ProviderSettings {
  api_key: string;
  base_url: string;
}

export interface ModelRef {
  provider: string;
  model: string;
}

export interface AiSettings {
  hotkey_ai: string;
  hotkey_cycle: string;
  providers: Record<string, ProviderSettings>;
  item: ModelRef;
  guide: ModelRef;
  cycle: ModelRef[];
  effort: "low" | "medium" | "high" | string;
  catalog: Record<string, ModelInfo[]>;
}

export interface Settings {
  hotkey: string;
  any_window: boolean;
  card_seconds: number;
  debug_captures: boolean;
  ocr_engine: string;
  ai: AiSettings;
}

export interface Fact {
  key: string;
  label: string;
  kind: "flag" | "counter" | string;
  default_on: boolean;
  default_count: number;
  note: string;
}

export interface BuildSummary {
  id: string;
  character_name: string;
  build_name: string;
  source: string;
  updated: number;
  model: string;
  profile_name: string;
  stat_count: number;
  facts: Fact[];
  builtin: boolean;
  active: boolean;
  guide_hash: string;
}

export interface CharacterState {
  level: number;
  phase: "early" | "intermediate" | "final" | null;
  resistances: Record<string, number>;
  endurance: number;
  health: number;
  mana: number;
  heavens_bulwark_points: number;
  healing_hands_specced: boolean;
  solarum_plate_equipped: boolean;
  nagasa_scymitar_equipped: boolean;
  flags?: Record<string, boolean>;
  counters?: Record<string, number>;
}

export interface ModelInfo {
  id: string;
  display_name: string;
  input_price: number | null;
  output_price: number | null;
  item_cost: number | null;
  guide_cost: number | null;
}

export interface GuideOutcome {
  build: BuildSummary;
  reused: boolean;
  usage: string;
}
