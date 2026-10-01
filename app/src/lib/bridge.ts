import type {
  HandoffItem,
  HandoffPlan,
  RevealError,
  RevealGroup,
  RevealSummary,
  Report,
  ScanConfig,
  ScanEvent,
  SkippedPath,
  Tier,
} from "./types";
import { MOCK_REPORT, mockScanEvents } from "./mock";

/**
 * The only seam between the frontend and the world.
 *
 * Inside the Tauri shell these map to Rust commands (see app/src-tauri):
 *   check_full_disk_access() -> "granted" | "denied" | "unknown"
 *   check_automation_permission() -> same — one Apple event to Finder;
 *                                    the first probe makes macOS ask
 *   open_system_settings()
 *   open_automation_settings()
 *   start_scan(config)            — emits `scan-event` windows
 *   cancel_scan()
 *   last_report() -> Report | null
 *   correct_tier(id, tier, note?)
 *   reveal_path(path)             — `open -R <path>`, single item only
 *   plan_handoff(items) -> HandoffPlan
 *                                 — the pre-commit view: containers, rollup,
 *                                   findings-vs-total, vanished paths
 *   reveal_paths(items, folders?) -> RevealSummary
 *                                 — one Finder window per container with the
 *                                   findings preselected, selection counts
 *                                   verified by reading back from Finder
 * In the browser (and until the shell is wired) a mock bridge answers, so
 * every screen is fully reviewable in `npm run dev`.
 */

export type FdaStatus = "granted" | "denied" | "unknown";

/** The river's second opinion for one candidate. Mirrors core's
 *  `Adjudication` (engine.rs) — the card's tier is never changed by it. */
export interface AdjudicationResult {
  candidate_id: string;
  card_tier: Tier;
  llm_tier: Tier;
  agrees: boolean;
  llm_safer: boolean;
  confidence: number;
  summary: string;
  reasoning: string;
  cost_usd: number;
  model: string;
  tier_source: "adjudication";
}

export interface AdjudicateOptions {
  model: string;
  apiKey?: string;
  allowNonZdr: boolean;
}

export interface Bridge {
  readonly name: string;
  checkFullDiskAccess(): Promise<FdaStatus>;
  /** Probe macOS's Automation consent for Finder control. The first probe
   *  makes macOS show its consent dialog; the answer may take a while. */
  checkAutomationPermission(): Promise<FdaStatus>;
  openSystemSettings(): void;
  openAutomationSettings(): void;
  startScan(cfg: ScanConfig, onEvent: (e: ScanEvent) => void): Promise<Report>;
  cancelScan(): void;
  getLastReport(): Promise<Report | null>;
  correctTier(id: string, tier: Tier): Promise<void>;
  /** On-demand adjudication: ask the river about one candidate. Never
   *  changes the card; the result is displayed as a second opinion. */
  adjudicate(id: string, opts: AdjudicateOptions): Promise<AdjudicationResult>;
  reveal(path: string): void;
  /** The pre-commit view of a hand-off: container folders with findings
   *  versus total, taint flags, and already-vanished paths — no Finder. */
  planHandoff(items: HandoffItem[]): Promise<HandoffPlan>;
  /** Hand a selection off to Finder: one window per container with the
   *  findings preselected, verified by reading the selection back from
   *  Finder. `folders` restricts the hand-off to specific containers (the
   *  per-folder actions pass one); undefined hands off everything.
   *  Rejects with a RevealError ({"kind": "automation_denied"} when macOS
   *  refuses the Finder consent). */
  revealAll(items: HandoffItem[], folders?: string[]): Promise<RevealSummary>;
}

const w = () => window as unknown as {
  __TAURI_INTERNALS__?: { invoke: (cmd: string, args?: unknown) => Promise<unknown> };
};

function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const internals = w().__TAURI_INTERNALS__;
  if (!internals) return Promise.reject(new Error("not inside the Tauri shell"));
  return internals.invoke(cmd, args) as Promise<T>;
}

class TauriBridge implements Bridge {
  readonly name = "tauri";
  async checkFullDiskAccess(): Promise<FdaStatus> {
    return tauriInvoke<FdaStatus>("check_full_disk_access");
  }
  async checkAutomationPermission(): Promise<FdaStatus> {
    return tauriInvoke<FdaStatus>("check_automation_permission");
  }
  openSystemSettings(): void {
    void tauriInvoke("open_system_settings");
  }
  openAutomationSettings(): void {
    void tauriInvoke("open_automation_settings");
  }
  async startScan(cfg: ScanConfig, onEvent: (e: ScanEvent) => void): Promise<Report> {
    const { listen } = await import("./tauri-events");
    const off = await listen<ScanEvent>("scan-event", onEvent);
    try {
      return await tauriInvoke<Report>("start_scan", { config: cfg });
    } finally {
      off?.();
    }
  }
  cancelScan(): void {
    void tauriInvoke("cancel_scan");
  }
  async getLastReport(): Promise<Report | null> {
    return tauriInvoke<Report | null>("last_report");
  }
  async correctTier(id: string, tier: Tier): Promise<void> {
    await tauriInvoke("correct_tier", { id, tier });
  }
  async adjudicate(
    id: string,
    opts: AdjudicateOptions,
  ): Promise<AdjudicationResult> {
    return tauriInvoke<AdjudicationResult>("adjudicate_candidate", {
      request: {
        id,
        model: opts.model,
        api_key: opts.apiKey,
        allow_non_zdr: opts.allowNonZdr,
      },
    });
  }
  reveal(path: string): void {
    void tauriInvoke("reveal_path", { path });
  }
  planHandoff(items: HandoffItem[]): Promise<HandoffPlan> {
    return tauriInvoke<HandoffPlan>("plan_handoff", { items });
  }
  revealAll(items: HandoffItem[], folders?: string[]): Promise<RevealSummary> {
    return tauriInvoke<RevealSummary>("reveal_paths", { items, folders });
  }
}

class MockBridge implements Bridge {
  readonly name = "mock";
  private cancelled = false;
  private failScan = false;
  /** `?reveal-snag`: simulate a vanished path, a Finder selection mismatch,
   *  and a group whose selection failed — the honest-reporting paths. */
  private failReveal = false;
  /** `?reveal-denied`: simulate macOS refusing the Finder automation. */
  private denyReveal = false;

  constructor() {
    if (typeof window !== "undefined") {
      const q = new URLSearchParams(window.location.search);
      this.failScan = q.has("snag");
      this.failReveal = q.has("reveal-snag");
      this.denyReveal = q.has("reveal-denied");
    }
  }

  async checkFullDiskAccess(): Promise<FdaStatus> {
    await sleep(1_100);
    return "denied";
  }
  openSystemSettings(): void {
    /* mock — would deep-link to x-apple.systempreferences */
  }
  async startScan(cfg: ScanConfig, onEvent: (e: ScanEvent) => void): Promise<Report> {
    this.cancelled = false;
    const script = mockScanEvents(cfg, this.failScan);
    let elapsed = 0;
    for (const [t, event] of script) {
      const wait = t - elapsed;
      if (wait > 0) await sleep(wait);
      elapsed = t;
      if (this.cancelled) throw new Error("cancelled");
      onEvent(event);
      if (event.type === "error") throw new Error(event.message);
    }
    if (this.cancelled) throw new Error("cancelled");
    return MOCK_REPORT;
  }
  cancelScan(): void {
    this.cancelled = true;
  }
  async getLastReport(): Promise<Report | null> {
    return MOCK_REPORT;
  }
  async correctTier(): Promise<void> {
    await sleep(500);
  }
  /**
   * Mock adjudication: the second opinion disagrees (and lands on the
   * safer side) for entries whose own stamp was unargued — the exact
   * failure adjudication exists to surface — and agrees otherwise.
   * Never touches the card.
   */
  async adjudicate(
    id: string,
    opts: AdjudicateOptions,
  ): Promise<AdjudicationResult> {
    await sleep(1_400);
    const entry = MOCK_REPORT.entries.find((e) => e.candidate.id === id);
    const card = (entry?.tier ?? 3) as Tier;
    const unargued =
      entry?.tier_source === "argued_auto_high" ||
      entry?.tier_source === "auto_high" ||
      entry?.tier_source === "fallback" ||
      entry?.tier_source === "heuristic";
    const llmTier: Tier = unargued ? Math.min(4, card + 2) as Tier : card;
    return {
      candidate_id: id,
      card_tier: card,
      llm_tier: llmTier,
      agrees: llmTier === card,
      llm_safer: llmTier > card,
      confidence: unargued ? 0.72 : 0.9,
      summary: entry?.summary ?? "s",
      reasoning: unargued
        ? "The strongest case for keeping this folder is that a live process may still reference it and its contents are not trivially regenerable. That counterargument holds: this is not the open-and-shut cache the card's heuristic assumed. I would raise it — the location and score alone do not make something safe to clear."
        : "The stamp survives scrutiny: this is regenerable data in a cache location with no configuration or personal state, and the owning app's own eviction logic manages it. Clearing costs only the refill.",
      cost_usd: 0.0021,
      model: opts.model || "mock",
      tier_source: "adjudication",
    };
  }
  reveal(): void {
    /* mock — would run `open -R <path>` */
  }
  async checkAutomationPermission(): Promise<FdaStatus> {
    await sleep(600);
    // Denied by default so the calm consent flow stays reviewable.
    return "denied";
  }
  openAutomationSettings(): void {
    /* mock — would deep-link to Privacy & Security → Automation */
  }
  async planHandoff(items: HandoffItem[]): Promise<HandoffPlan> {
    await sleep(250);
    const { kept, skipped } = this.snagDrop(items);
    const groups = rollupGroups(groupItems(kept), MOCK_ROLLUP_TARGET);
    return {
      groups: groups.map((g) => ({
        folder: g.folder,
        findings: g.items.length,
        finding_bytes: bytesOf(g),
        total_in_folder: mockFolderTotal(g.folder, g.items.length),
        tainted: g.tainted,
      })),
      skipped,
      windows: groups.length,
    };
  }
  async revealAll(items: HandoffItem[], folders?: string[]): Promise<RevealSummary> {
    await sleep(700);
    if (this.denyReveal) {
      throw { kind: "automation_denied" } satisfies RevealError;
    }
    const { kept, skipped } = this.snagDrop(items);
    let groups = rollupGroups(groupItems(kept), MOCK_ROLLUP_TARGET);
    if (folders) {
      const wanted = new Set(folders);
      const keptGroups: MockGroup[] = [];
      for (const g of groups) {
        if (wanted.has(g.folder)) {
          keptGroups.push(g);
        } else {
          for (const i of g.items) {
            skipped.push({
              path: i.path,
              reason: "outside the folders you picked for this hand-off",
            });
          }
        }
      }
      groups = keptGroups;
    }
    const last = groups.length - 1;
    const out: RevealGroup[] = groups.map((g, i) => {
      // The snag: the first window "drops" one item from its selection, and
      // the last window's selection fails outright — the summary must show
      // both exactly as Finder would have reported them.
      const mismatch = this.failReveal && i === 0 && g.items.length > 1;
      const failed = this.failReveal && i === last && groups.length > 1 && i !== 0;
      const selected = failed ? 0 : mismatch ? g.items.length - 1 : g.items.length;
      const error = failed
        ? "Finder could not set the selection (-1712): Finder got an error: AppleEvent timed out."
        : undefined;
      return {
        folder: g.folder,
        findings: g.items.length,
        finding_bytes: bytesOf(g),
        total_in_folder: mockFolderTotal(g.folder, g.items.length),
        requested: g.items.length,
        selected,
        tainted: g.tainted,
        ok: error === undefined && selected === g.items.length,
        ...(error !== undefined ? { error } : {}),
      };
    });
    return {
      groups: out,
      skipped,
      windows: out.length,
      items_requested: out.reduce((n, g) => n + g.requested, 0),
      items_selected: out.reduce((n, g) => n + g.selected, 0),
    };
  }
  /** With the snag flag, one path vanishes between scan and hand-off —
   *  exactly what happens when an owning app clears its own cache. */
  private snagDrop(items: HandoffItem[]): {
    kept: HandoffItem[];
    skipped: SkippedPath[];
  } {
    if (!this.failReveal || items.length < 2) return { kept: items, skipped: [] };
    const vanished = items[1];
    return {
      kept: items.filter((_, i) => i !== 1),
      skipped: [
        {
          path: vanished?.path ?? "",
          reason: "vanished since the scan — already gone from disk",
        },
      ],
    };
  }
}

/* ---------- mock hand-off machinery ----------
 * Mirrors the shell's grouping so the browser review shows the same
 * containers the real hand-off would: parent grouping, descendant fold on
 * component boundaries, guarded rollup into the scan roots, byte-descending
 * order, and the taint flag. Purely read-only simulation. */

const MOCK_HOME = "/Users/you";
const MOCK_SCAN_ROOTS = [
  `${MOCK_HOME}/Library/Caches`,
  "/tmp",
  `${MOCK_HOME}/Library/Logs`,
  `${MOCK_HOME}/Library/Application Support`,
  `${MOCK_HOME}/Downloads`,
  `${MOCK_HOME}/Library/Containers`,
  `${MOCK_HOME}/Documents`,
  `${MOCK_HOME}/Pictures`,
  `${MOCK_HOME}/Movies`,
  `${MOCK_HOME}/Music`,
];
const MOCK_ROLLUP_TARGET = 12;

interface MockGroup {
  folder: string;
  items: HandoffItem[];
  tainted: boolean;
}

function bytesOf(g: MockGroup): number {
  return g.items.reduce((n, i) => n + i.size_bytes, 0);
}

function parentOf(path: string): string {
  return path.split("/").slice(0, -1).join("/");
}

/** Component-boundary comparison, mirroring the shell's `Path::starts_with`:
 *  /foo/barbaz is never inside /foo/bar. */
function isProperAncestor(anc: string, desc: string): boolean {
  if (anc === desc) return false;
  const a = anc.split("/").filter(Boolean);
  const d = desc.split("/").filter(Boolean);
  return a.length < d.length && a.every((c, i) => d[i] === c);
}

function isTainted(item: HandoffItem): boolean {
  return item.tier >= 4 || (item.scope !== "low" && item.scope !== "medium");
}

function groupItems(items: HandoffItem[]): MockGroup[] {
  const byParent = new Map<string, MockGroup>();
  for (const item of items) {
    const parent = parentOf(item.path);
    if (!parent) continue;
    let g = byParent.get(parent);
    if (!g) byParent.set(parent, (g = { folder: parent, items: [], tainted: false }));
    g.tainted = g.tainted || isTainted(item);
    g.items.push(item);
  }
  // Fold descendants into ancestors until stable, like the shell.
  for (;;) {
    const folders = [...byParent.keys()];
    let fold: [string, string] | undefined;
    outer: for (const f of folders) {
      for (const o of folders) {
        if (o !== f && isProperAncestor(o, f)) {
          fold = [f, o];
          break outer;
        }
      }
    }
    if (!fold) break;
    const [from, into] = fold;
    const folded = byParent.get(from)!;
    byParent.delete(from);
    const target = byParent.get(into)!;
    target.tainted = target.tainted || folded.tainted;
    target.items.push(...folded.items);
  }
  return sortGroups([...byParent.values()]);
}

function sortGroups(groups: MockGroup[]): MockGroup[] {
  return groups.sort(
    (a, b) => bytesOf(b) - bytesOf(a) || (a.folder < b.folder ? -1 : 1),
  );
}

/** Same guards as the shell: never into a finding, never across a taint
 *  boundary, never outside the scanned roots — and only while over target. */
function rollupGroups(groups: MockGroup[], target: number): MockGroup[] {
  let current = groups;
  while (current.length > target) {
    let best: { bytes: number; folder: string } | undefined;
    for (const candidate of MOCK_SCAN_ROOTS) {
      if (current.some((g) => g.items.some((i) => i.path === candidate))) continue;
      let covers = 0;
      let mergesAny = false;
      let tainted = false;
      for (const g of current) {
        if (g.folder === candidate || isProperAncestor(candidate, g.folder)) {
          if (g.tainted) tainted = true;
          covers += bytesOf(g);
          if (g.folder !== candidate) mergesAny = true;
        }
      }
      if (tainted || !mergesAny) continue;
      if (!best || covers > best.bytes) best = { bytes: covers, folder: candidate };
    }
    if (!best) break;
    const ancestor = best.folder;
    const merged: MockGroup = { folder: ancestor, items: [], tainted: false };
    const kept: MockGroup[] = [];
    for (const g of current) {
      if (g.folder === ancestor || isProperAncestor(ancestor, g.folder)) {
        merged.tainted = merged.tainted || g.tainted;
        merged.items.push(...g.items);
      } else {
        kept.push(g);
      }
    }
    kept.push(merged);
    current = sortGroups(kept);
  }
  return current;
}

/** Simulated directory entry count: deterministic per folder, sometimes
 *  unknown, always at least the findings count — crowded enough that the
 *  findings-versus-total warning is reviewable in the browser. */
function mockFolderTotal(folder: string, findings: number): number | null {
  let h = 0;
  for (let i = 0; i < folder.length; i++) h = (h * 31 + folder.charCodeAt(i)) >>> 0;
  if (h % 7 === 0) return null;
  return findings * (1 + (h % 12) * 5);
}

function sleep(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}

export const bridge: Bridge =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window
    ? new TauriBridge()
    : new MockBridge();
