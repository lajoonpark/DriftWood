/* Mirror of crates/driftwood-core/src/types.rs — the wire contract.
   Field names must stay identical to the serialized Rust model. */

export type Tier = 1 | 2 | 3 | 4;

export type TierSource =
  | "auto_high"
  | "argued_auto_high"
  | "auto_low"
  | "rule"
  | "llm"
  | "llm_propagated"
  | "fallback"
  | "heuristic"
  | "never_flag"
  | "system_floor"
  | "adjudication"
  | "not_inspected";

export type Kind = "file" | "folder" | "app";
export type OrphanStatus = "orphaned" | "active" | "unknown";
export type ScopeCategory = "low" | "medium" | "high";
export type PrivacyTier = "minimal" | "standard" | "deep";
export type Band = "high" | "middle" | "low";

/** Tri-state read state for a collected field (size, child count, a date).
 *  `"known"` carries a real value; `"unavailable"` means no data source
 *  existed; `{ error }` means the read failed (e.g. permission denied).
 *  Never render a non-known field as 0 / empty. */
export type FieldState = "known" | "unavailable" | { error: string };

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
  /** Tri-state read state for each collected field. Missing on reports
   *  persisted before this field existed → treat as "known". */
  size_state?: FieldState;
  children_state?: FieldState;
  modified_state?: FieldState;
  created_state?: FieldState;
  last_used_state?: FieldState;
  /** False when sizing or listing failed, or was partial. Such items are
   *  held at Source by the backend and never sent to the model. */
  readable?: boolean;
  /** Human-readable reason when readable is false (e.g. "permission denied"). */
  read_error?: string;
  orphan_status: OrphanStatus;
  scope_category: ScopeCategory;
  score: number;
  score_components: ScoreComponents;
  band: Band;
  /** Why auto-high promoted this candidate (argued auto-high). */
  auto_high_basis?: "cache_root" | "orphaned_app_support" | "quantile_band";
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
  /** Stage 2 spend during the scan itself. */
  llm_cost_usd?: number;
  /** Cumulative spend of on-demand adjudications made after the scan.
   *  Adjudication cost is never folded into llm_cost_usd — the scan total
   *  stays the scan total — but the report total must include both. */
  adjudication_cost_usd?: number;
}

/* ---------- Finder hand-off ---------- */

/** One selected finding as sent to the hand-off planner/executor. Tier and
 *  personal-risk scope travel with the path so a container can be judged by
 *  what it aggregates, not just by the leaves inside it. */
export interface HandoffItem {
  path: string;
  size_bytes: number;
  tier: Tier;
  scope: ScopeCategory;
}

/** A path that could not be handed off, and why. Nothing is ever dropped
 *  silently: every selected path ends up in a group or in a list like this. */
export interface SkippedPath {
  path: string;
  reason: string;
}

/** One container folder the hand-off opens, with what Finder actually
 *  reported as selected. `selected` is a Finder readback, not our request —
 *  a mismatch is surfaced, never hidden, and `ok` comes only from that
 *  readback, never from the mere absence of an error. */
export interface RevealGroup {
  folder: string;
  findings: number;
  finding_bytes: number;
  /** Directory entry count of the container, null when it cannot be
   *  counted cheaply — rendered as "total unknown", never as zero. */
  total_in_folder: number | null;
  requested: number;
  selected: number;
  /** Contains a Source-tier or high-personal-risk finding. */
  tainted: boolean;
  ok: boolean;
  error?: string;
}

/** What a bulk Finder hand-off actually did (from reveal_paths), verified
 *  by reading the selection count back out of Finder. */
export interface RevealSummary {
  groups: RevealGroup[];
  skipped: SkippedPath[];
  windows: number;
  items_requested: number;
  /** Finder-verified total across all groups. */
  items_selected: number;
}

/** The pre-commit view of one container folder (from plan_handoff): the
 *  exact grouping and rollup the execution will do, with counts, before
 *  the user commits. */
export interface PlanGroup {
  folder: string;
  findings: number;
  finding_bytes: number;
  total_in_folder: number | null;
  tainted: boolean;
}

export interface HandoffPlan {
  groups: PlanGroup[];
  skipped: SkippedPath[];
  windows: number;
}

/** Why a hand-off failed outright. `automation_denied` is macOS's refusal
 *  of Finder control (TCC error -1743) — it gets its own calm explanation,
 *  never a raw osascript error on a dead button. */
export type RevealError =
  | { kind: "automation_denied" }
  | { kind: "script"; message: string };

/** How much of the river to run. Mirrors core's `ScanMode`. */
export type ScanMode = "express" | "standard" | "deep_read";

export const SCAN_MODE_LABELS: Record<ScanMode, string> = {
  express: "Express",
  standard: "Standard",
  deep_read: "Deep read",
};

export interface ScanConfig {
  scopes: ScopeCategory[];
  privacy_tier: PrivacyTier;
  /** Express: no AI. Standard: the middle band is argued. Deep read:
   *  bands are advisory — every surviving candidate is argued (expensive;
   *  clustering keeps it affordable). The system floor still wins. */
  mode: ScanMode;
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
