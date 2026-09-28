import { writable, get } from "svelte/store";
import type {
  PrivacyTier,
  Report,
  ScanConfig,
  ScanEvent,
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
  expressScan: false,
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
  /** Express Scan: skip AI reasoning entirely — heuristic tiers only,
   *  instant and free. Honest about it in the report. */
  expressScan: boolean;
  /** Danger zone: when false (default), every Stage-2 call is routed only
   *  to zero-data-retention providers. When true, OpenRouter may route to
   *  any provider — required for most free models, not recommended. */
  allowNonZdr: boolean;
}

function loadSettings(): AppSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (raw) return { ...DEFAULT_SETTINGS, apiKey: "", ...(JSON.parse(raw) as Partial<AppSettings>) };
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

export interface ScanState {
  running: boolean;
  phase: Phase | null;
  filesSearched: number;
  bytesSearched: number;
  candidates: number;
  recoverableBytes: number;
  feed: FeedLine[];
  error: string | null;
  report: Report | null;
}

const initialScan: ScanState = {
  running: false,
  phase: null,
  filesSearched: 0,
  bytesSearched: 0,
  candidates: 0,
  recoverableBytes: 0,
  feed: [],
  error: null,
  report: null,
};

function createScanStore() {
  const { subscribe, set, update } = writable<ScanState>(initialScan);

  return {
    subscribe,
    reset: () => set({ ...initialScan, report: get(scan).report }),
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
            return { ...s, recoverableBytes: Math.max(s.recoverableBytes, e.total) };
          case "notice":
          case "warn":
            return { ...s, feed: [...s.feed, e as FeedLine] };
          case "error":
            return { ...s, error: e.message, running: false };
        }
      }),
    setRunning: (running: boolean) => update((s) => ({ ...s, running })),
    setReport: (report: Report) => update((s) => ({ ...s, report, running: false })),
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
