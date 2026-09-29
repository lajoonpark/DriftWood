/* Mirror of crates/driftwood-core/src/types.rs — the wire contract.
   Field names must stay identical to the serialized Rust model. */

export type Tier = 1 | 2 | 3 | 4;

export type TierSource =
  | "auto_high"
  | "auto_low"
  | "rule"
  | "llm"
  | "llm_propagated"
  | "fallback"
  | "never_flag";

export type Kind = "file" | "folder" | "app";
export type OrphanStatus = "orphaned" | "active" | "unknown";
export type ScopeCategory = "low" | "medium" | "high";
export type PrivacyTier = "minimal" | "standard" | "deep";
export type Band = "high" | "middle" | "low";

/** Core scan phases, mapped to themed labels in the UI. */
export type Phase =
  | "enumerating"
  | "wading"
  | "filtering"
  | "scoring"
  | "reasoning"
  | "assembling";

export const TIER_NAMES: Record<Tier, string> = {
  1: "Driftwood",
  2: "Message in a Bottle",
  3: "Current",
  4: "Source",
};

export const TIER_BLURBS: Record<Tier, string> = {
  1: "No risk at all — dead wood, safe to clear.",
  2: "Disposable, but worth a look before tossing.",
  3: "Recoverable, but reacquiring it would cost you.",
  4: "Personal or irreplaceable — don't touch.",
};

export const PHASE_LABELS: Record<Phase, string> = {
  enumerating: "Searching for driftwood",
  wading: "Wading in",
  filtering: "Following the current",
  scoring: "Scoring",
  reasoning: "Traveling to the river",
  assembling: "Sorting the driftwood",
};

export const SCOPE_LABELS: Record<ScopeCategory, string> = {
  low: "Low personal risk",
  medium: "Medium personal risk",
  high: "High personal risk",
};

export const SCOPE_HINTS: Record<ScopeCategory, string> = {
  low: "Caches, logs, temp files — regenerates itself if cleared.",
  medium: "Downloads and sandboxed app containers. Mostly replaceable.",
  high: "Documents, photos, music. Only drift here on purpose.",
};

export const PRIVACY_LABELS: Record<PrivacyTier, string> = {
  minimal: "Minimal",
  standard: "Standard",
  deep: "Deep",
};

export interface KindStats {
  children: number;
  files: number;
  cache_like_ratio: number;
  truncated: boolean;
}

export interface ScoreComponents {
  size: number;
  age: number;
  cache_loc: number;
  orphan: number;
  depth: number;
  file_type: number;
  child_count: number;
}

export interface Candidate {
  id: string;
  path: string;
  kind: Kind;
  size_bytes: number;
  kind_stats?: KindStats;
  last_used_date?: string;
  last_used_from_spotlight: boolean;
  modified_date?: string;
  created_date?: string;
  orphan_status: OrphanStatus;
  scope_category: ScopeCategory;
  score: number;
  score_components: ScoreComponents;
  band: Band;
}

export interface ReportEntry {
  candidate: Candidate;
  tier: Tier;
  tier_source: TierSource;
  summary: string;
  reasoning: string;
  confidence: number;
  llm_model?: string;
  privacy_tier_used: PrivacyTier;
  rule_id?: string;
}

export interface ReportGroup {
  category: ScopeCategory;
  count: number;
  bytes: number;
}

export interface Report {
  groups: ReportGroup[];
  entries: ReportEntry[];
  /** Non-fatal notices carried with the report (e.g. murky Spotlight data). */
  warnings?: string[];
  /** Set when the cost cap cut Stage 2 short ("Snagged — ran out of river"). */
  cost_cap?: boolean;
  /** Set when the user pulled the scan ashore mid-crossing. Distinct from
   *  a cost-cap stop; the report is partial, unjudged items are fallback. */
  stopped_early?: boolean;
}

/** What a bulk Finder hand-off actually did (from reveal_paths). */
export interface RevealSummary {
  windows: number;
  items: number;
  skipped_groups: number;
  skipped_items: number;
}

export interface ScanConfig {
  scopes: ScopeCategory[];
  privacy_tier: PrivacyTier;
  stage2: boolean;
  model?: string;
  /** OpenRouter API key from app settings; stays local. */
  api_key?: string;
  cost_cap_usd?: number;
  /** Danger zone: when true, Stage-2 calls are not restricted to
   *  zero-data-retention providers (needed for most free models). */
  allow_non_zdr?: boolean;
}

export type ScanEvent =
  | { type: "phase"; phase: Phase }
  | { type: "files_searched"; total: number }
  | { type: "bytes_searched"; total: number }
  | { type: "candidates_found"; total: number }
  | { type: "recoverable_bytes"; total: number }
  /** Stage 2 progress: judged/total are real item counts (cluster
   *  propagation included); cost/tokens are actual spend so far — only
   *  finalized when a streamed call ends. Reasoning is the only phase
   *  with a knowable denominator; never render a percentage elsewhere. */
  | {
      type: "reasoning_progress";
      judged: number;
      total: number;
      cost_usd: number;
      prompt_tokens: number;
      completion_tokens: number;
    }
  | { type: "batch_started"; index: number; total_batches: number }
  | { type: "batch_finished"; index: number; total_batches: number }
  | { type: "notice"; message: string }
  | { type: "warn"; message: string }
  | { type: "error"; message: string };
