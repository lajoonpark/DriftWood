import { writable, get } from "svelte/store";
import type {
  PrivacyTier,
  Report,
  ScanConfig,
  ScanEvent,
  ScanMode,
  ScopeCategory,
  Phase,
} from "./types";

export type View =
  | "welcome"
  | "scopes"
  | "trust"
  | "privacy"
  | "scan"
  | "report"
  | "browser"
  | "settings";

/* ---------- persistent app state ---------- */

const LS_KEY = "driftwood.app.v1";

export interface OnboardingState {
  scopes: Record<ScopeCategory, boolean>;
  folders: Record<string, boolean>;
  privacy: PrivacyTier;
  done: boolean;
}

export const SCOPE_FOLDERS: Record<ScopeCategory, { path: string; hint: string }[]> = {
  low: [
    { path: "~/Library/Caches", hint: "App caches" },
    { path: "/tmp", hint: "Temp files" },
    { path: "~/Library/Logs", hint: "Logs" },
    { path: "~/Library/Application Support (orphaned)", hint: "Left by uninstalled apps" },
  ],
  medium: [
    { path: "~/Downloads", hint: "Installers, archives, one-off files" },
    { path: "~/Library/Containers", hint: "Sandboxed app data" },
  ],
  high: [
    { path: "~/Documents", hint: "Your documents" },
    { path: "~/Pictures", hint: "Photos" },
    { path: "~/Movies", hint: "Video" },
    { path: "~/Music", hint: "Music" },
  ],
};

export const DEFAULT_SETTINGS = {
  model: "anthropic/claude-haiku-4.5",
  costCap: 0.5,
  scanMode: "standard" as ScanMode,
  allowNonZdr: false,
};

/* ---------- persisted tuning (model, cost cap, OpenRouter key) ---------- */

const SETTINGS_KEY = "driftwood.settings.v1";

export interface AppSettings {
  /** OpenRouter model id, e.g. "anthropic/claude-haiku-class". */
  model: string;
  /** Hard ceiling per scan, USD. */
  costCap: number;
  /** OpenRouter API key (sk-or-…). Stays on this machine. */
  apiKey: string;
  /** How much of the river to run: express (free, heuristic tiers only),
   *  standard (middle band argued), deep read (everything argued). */
  scanMode: ScanMode;
  /** Danger zone: when false (default), every Stage-2 call is routed only
   *  to zero-data-retention providers. When true, OpenRouter may route to
   *  any provider — required for most free models, not recommended. */
  allowNonZdr: boolean;
}

function loadSettings(): AppSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<AppSettings> & {
        expressScan?: boolean;
      };
      /* Migration from the old expressScan boolean (same localStorage key,
         same merge shape — old installs keep everything else they saved):
           expressScan: true  → Express (they opted out of AI before)
           expressScan: false → Standard (they ran full Stage 2 before;
                                        deliberately NOT moved to the new
                                        third mode — that would be a silent
                                        behavior change for existing users)
         A saved scanMode always wins. */
      const migrated: ScanMode = parsed.scanMode ??
        (parsed.expressScan === true ? "express" : "standard");
      return {
        ...DEFAULT_SETTINGS,
        apiKey: "",
        ...parsed,
        scanMode: migrated,
      };
    }
  } catch {
    /* fresh start */
  }
  return { ...DEFAULT_SETTINGS, apiKey: "" };
}

export const appSettings = writable<AppSettings>(loadSettings());

export function saveSettings(s: AppSettings) {
  const p = plain(s);
  appSettings.set(p);
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(p));
  } catch {
    /* private mode */
  }
}

export function resetEverything() {
  try {
    localStorage.removeItem(LS_KEY);
    localStorage.removeItem(SETTINGS_KEY);
  } catch {
    /* ignore */
  }
  location.reload();
}

function loadOnboarding(): OnboardingState {
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (raw) return JSON.parse(raw) as OnboardingState;
  } catch {
    /* fresh start */
  }
  return {
    scopes: { low: true, medium: false, high: false },
    folders: {
      "~/Library/Caches": true,
      "/tmp": true,
      "~/Library/Logs": true,
      "~/Library/Application Support (orphaned)": true,
    },
    privacy: "standard",
    done: false,
  };
}

export const onboarding = writable<OnboardingState>(loadOnboarding());

/** Screens pass $state proxies around; never let one into the store —
 *  a proxy stored here makes the next `structuredClone($onboarding)`
 *  (Settings, Scopes) throw DataCloneError and the navigation dies. */
function plain<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T;
}

export function saveOnboarding(s: OnboardingState) {
  const p = plain(s);
  onboarding.set(p);
  try {
    localStorage.setItem(LS_KEY, JSON.stringify(p));
  } catch {
    /* private mode */
  }
}

export function markOnboarded() {
  const s = get(onboarding);
  saveOnboarding({ ...s, done: true });
}

/* ---------- runtime scan state ---------- */

export type FeedLine = { type: "notice" | "warn"; message: string };

/** Live Stage 2 state, fed by reasoning_progress / batch events. Only the
 *  Reasoning phase has a knowable denominator — the UI must not render a
 *  percentage for the walk/sizing phases. Speed and ETA are EMAs computed
 *  here so they visibly settle instead of swinging on the first batches. */
export interface ReasoningState {
  judged: number;
  total: number;
  costUsd: number;
  promptTokens: number;
  completionTokens: number;
  batchesStarted: number;
  batchesFinished: number;
  totalBatches: number;
  /* runtime-derived, not part of the wire contract */
  startedAt: number;
  lastBatchAt: number;
  lastProgressAt: number;
  lastJudged: number;
  /** items/sec, EMA */
  speed: number;
  /** ms per batch, EMA (ETA denominator = remaining batches) */
  batchEmaMs: number;
}

export interface ScanState {
  running: boolean;
  /** Set when the user clicks Pull ashore; stays until the scan resolves. */
  stopping: boolean;
  phase: Phase | null;
  filesSearched: number;
  bytesSearched: number;
  candidates: number;
  recoverableBytes: number;
  reasoning: ReasoningState | null;
  feed: FeedLine[];
  error: string | null;
  report: Report | null;
  /** Session-cumulative adjudication spend on top of the report's own
   *  Stage 2 total — the backend also folds it into the persisted report,
   *  but the in-memory copy loaded earlier would otherwise understate. */
  adjudicationCostUsd: number;
}

function initialReasoning(): ReasoningState {
  return {
    judged: 0,
    total: 0,
    costUsd: 0,
    promptTokens: 0,
    completionTokens: 0,
    batchesStarted: 0,
    batchesFinished: 0,
    totalBatches: 0,
    startedAt: 0,
    lastBatchAt: 0,
    lastProgressAt: 0,
    lastJudged: 0,
    speed: 0,
    batchEmaMs: 0,
  };
}

const initialScan: ScanState = {
  running: false,
  stopping: false,
  phase: null,
  filesSearched: 0,
  bytesSearched: 0,
  candidates: 0,
  recoverableBytes: 0,
  reasoning: null,
  feed: [],
  error: null,
  report: null,
  adjudicationCostUsd: 0,
};

/** EMA factor: heavy enough to settle after the first two batches,
 *  light enough to track a real change in pace. */
const EMA = 0.35;

function createScanStore() {
  const { subscribe, set, update } = writable<ScanState>(initialScan);

  return {
    subscribe,
    reset: () =>
      set({
        ...initialScan,
        report: get(scan).report,
        adjudicationCostUsd: get(scan).report?.adjudication_cost_usd ?? 0,
      }),
    setStopping: () => update((s) => ({ ...s, stopping: true })),
    applyEvent: (e: ScanEvent) =>
      update((s) => {
        switch (e.type) {
          case "phase":
            return { ...s, phase: e.phase };
          case "files_searched":
            return { ...s, filesSearched: Math.max(s.filesSearched, e.total) };
          case "bytes_searched":
            return { ...s, bytesSearched: Math.max(s.bytesSearched, e.total) };
          case "candidates_found":
            return { ...s, candidates: Math.max(s.candidates, e.total) };
          case "recoverable_bytes":
            // Take the latest, never the max: the counter is conservative
            // while tiers are still unknown and exact once assembly is
            // done — a correction downward is real information, and
            // Math.max would pin the stale overstated figure.
            return { ...s, recoverableBytes: e.total };
          case "reasoning_progress": {
            const now = Date.now();
            const r = s.reasoning ?? initialReasoning();
            const prevAt = r.lastProgressAt;
            const deltaItems = Math.max(0, e.judged - r.lastJudged);
            const rate =
              prevAt > 0 && now > prevAt
                ? deltaItems / ((now - prevAt) / 1000)
                : 0;
            return {
              ...s,
              reasoning: {
                ...r,
                judged: e.judged,
                total: e.total,
                costUsd: e.cost_usd,
                promptTokens: e.prompt_tokens,
                completionTokens: e.completion_tokens,
                lastProgressAt: now,
                lastJudged: e.judged,
                speed:
                  r.speed === 0 ? rate : EMA * rate + (1 - EMA) * r.speed,
              },
            };
          }
          case "batch_started": {
            const r = s.reasoning ?? initialReasoning();
            return {
              ...s,
              reasoning: {
                ...r,
                batchesStarted: r.batchesStarted + 1,
                totalBatches: Math.max(r.totalBatches, e.total_batches),
                startedAt: r.startedAt || Date.now(),
              },
            };
          }
          case "batch_finished": {
            const now = Date.now();
            const r = s.reasoning ?? initialReasoning();
            const duration =
              r.lastBatchAt > 0 && now > r.lastBatchAt ? now - r.lastBatchAt : 0;
            return {
              ...s,
              reasoning: {
                ...r,
                batchesFinished: r.batchesFinished + 1,
                totalBatches: Math.max(r.totalBatches, e.total_batches),
                lastBatchAt: now,
                batchEmaMs:
                  r.batchEmaMs === 0 || duration === 0
                    ? r.batchEmaMs || duration
                    : EMA * duration + (1 - EMA) * r.batchEmaMs,
              },
            };
          }
          case "notice":
          case "warn":
            return { ...s, feed: [...s.feed, e as FeedLine] };
          case "error":
            return { ...s, error: e.message, running: false };
        }
      }),
    setRunning: (running: boolean) =>
      update((s) => ({
        ...s,
        running,
        stopping: running ? false : s.stopping,
        reasoning: running ? null : s.reasoning,
      })),
    setReport: (report: Report) =>
      update((s) => ({
        ...s,
        report,
        running: false,
        stopping: false,
        adjudicationCostUsd: report.adjudication_cost_usd ?? 0,
      })),
    /** Restore the last persisted report (app relaunch) into an empty
     *  store. A store that already has a report — a fresh scan finished
     *  while the disk read was in flight — is never overwritten. */
    loadPersisted: (report: Report): boolean => {
      let applied = false;
      update((s) => {
        if (s.report) return s;
        applied = true;
        return {
          ...s,
          report,
          adjudicationCostUsd: report.adjudication_cost_usd ?? 0,
        };
      });
      return applied;
    },
    /** Adjudications spend after the scan; the honest report total is
     *  scan spend + this. The backend persists the same figure. */
    addAdjudicationCost: (cost: number) =>
      update((s) => ({ ...s, adjudicationCostUsd: s.adjudicationCostUsd + cost })),
  };
}

export const scan = createScanStore();

/* ---------- toasts ---------- */

export interface Toast {
  id: number;
  text: string;
}

export const toasts = writable<Toast[]>([]);
let toastId = 0;

export function toast(text: string) {
  const id = ++toastId;
  toasts.update((t) => [...t, { id, text }]);
  setTimeout(() => toasts.update((t) => t.filter((x) => x.id !== id)), 3_400);
}
