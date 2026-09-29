//! driftwood — headless CLI over driftwood-core. The Phase 1–2 workhorse:
//! `driftwood scan --scope low` prints a scored candidate JSON; tuning
//! flags make weight/threshold iteration cheap (plan Phase 2).

use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};
use driftwood_core::{
    apply_correction, run_scan, CorrectionRequest, DriftTuning, EventSink, OrphanStatus,
    Phase, ScanConfig, ScanEvent, ScanHandle, ScopeCategory, Tier,
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Scope {
    Low,
    Medium,
    High,
    All,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Privacy {
    Minimal,
    Standard,
    Deep,
}

#[derive(Parser)]
#[command(name = "driftwood", about = "Read-only macOS drift scanner — find what the river left behind.", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a scan and print the report JSON.
    Scan {
        #[arg(long, value_enum, default_value = "low")]
        scope: Scope,
        /// Run Stage 2 (LLM reasoning). Requires OPENROUTER_API_KEY.
        #[arg(long)]
        llm: bool,
        #[arg(long, value_enum, default_value = "standard")]
        privacy: Privacy,
        #[arg(long)]
        model: Option<String>,
        /// Override the per-scan USD cost cap.
        #[arg(long)]
        cost_cap: Option<f64>,
        /// TOML file overriding weights/thresholds.
        #[arg(long)]
        weights: Option<String>,
        /// Force absolute band cutoffs "high,low" (e.g. "75,25").
        #[arg(long)]
        band_cutoffs: Option<String>,
        /// Print a human-readable score-component table to stderr.
        #[arg(long)]
        dump_components: bool,
        /// Quiet: no progress output on stderr.
        #[arg(long)]
        quiet: bool,
    },
    /// Diff two report JSON files: tier changes and candidates that
    /// appeared/disappeared between runs.
    Diff {
        report_a: String,
        report_b: String,
    },
    /// Record a tier correction (feeds the preference learner).
    Correct {
        path: String,
        /// Original tier number 1..4 from the report.
        #[arg(long)]
        from: u8,
        /// Corrected tier number 1..4.
        #[arg(long)]
        to: u8,
        #[arg(long)]
        note: Option<String>,
        /// Candidate size in bytes (for feature extraction).
        #[arg(long)]
        size: Option<u64>,
    },
    /// Print the current distilled rules.
    Rules,
    /// Print the default tuning TOML (edit and pass via --weights).
    Tuning,
}

fn main() {
    let cli = Cli::parse();
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let code = match cli.command {
        Command::Scan {
            scope,
            llm,
            privacy,
            model,
            cost_cap,
            weights,
            band_cutoffs,
            dump_components,
            quiet,
        } => rt.block_on(cmd_scan(
            scope, llm, privacy, model, cost_cap, weights, band_cutoffs, dump_components, quiet,
        )),
        Command::Diff { report_a, report_b } => {
            cmd_diff(&report_a, &report_b)
        }
        Command::Correct { path, from, to, note, size } => {
            rt.block_on(cmd_correct(&path, from, to, note, size))
        }
        Command::Rules => cmd_rules(),
        Command::Tuning => cmd_tuning(),
    };
    std::process::exit(code);
}

fn scope_categories(scope: Scope) -> Vec<ScopeCategory> {
    match scope {
        Scope::Low => vec![ScopeCategory::Low],
        Scope::Medium => vec![ScopeCategory::Medium],
        Scope::High => vec![ScopeCategory::High],
        Scope::All => vec![ScopeCategory::Low, ScopeCategory::Medium, ScopeCategory::High],
    }
}

fn parse_privacy(p: Privacy) -> driftwood_core::PrivacyTier {
    match p {
        Privacy::Minimal => driftwood_core::PrivacyTier::Minimal,
        Privacy::Standard => driftwood_core::PrivacyTier::Standard,
        Privacy::Deep => driftwood_core::PrivacyTier::Deep,
    }
}

fn themed_phase(phase: Phase) -> &'static str {
    match phase {
        Phase::Enumerating => "Searching for driftwood",
        Phase::Wading => "Wading in",
        Phase::Filtering => "Following the current",
        Phase::Scoring => "Scoring",
        Phase::Reasoning => "Traveling to the river",
        Phase::Assembling => "Sorting the driftwood",
    }
}

struct CliSink {
    quiet: bool,
}

impl EventSink for CliSink {
    fn emit(&self, event: ScanEvent) {
        if self.quiet {
            return;
        }
        match event {
            ScanEvent::Phase { phase } => {
                eprintln!("==> {}", themed_phase(phase));
            }
            ScanEvent::FilesSearched { total } => {
                eprint!("\r files searched: {total}");
            }
            ScanEvent::BytesSearched { total } => {
                eprint!("\r bytes searched: {}", human_bytes(total));
            }
            ScanEvent::CandidatesFound { total } => {
                eprintln!("\rcandidates found: {total}");
            }
            ScanEvent::RecoverableBytes { total } => {
                eprintln!("recoverable: {}", human_bytes(total));
            }
            ScanEvent::ReasoningProgress {
                judged,
                total,
                cost_usd,
                ..
            } => {
                eprint!(
                    "\r reasoned: {judged}/{total} · {} so far",
                    driftwood_core::reason::format_cost(cost_usd)
                );
            }
            ScanEvent::BatchStarted { .. } | ScanEvent::BatchFinished { .. } => {}
            ScanEvent::Notice { message } => eprintln!("    · {message}"),
            ScanEvent::Warn { message } => eprintln!("    ! {message}"),
            ScanEvent::Error { message } => eprintln!("    x {message}"),
        }
    }
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[allow(clippy::too_many_arguments)]
async fn cmd_scan(
    scope: Scope,
    llm: bool,
    privacy: Privacy,
    model: Option<String>,
    cost_cap: Option<f64>,
    weights: Option<String>,
    band_cutoffs: Option<String>,
    dump_components: bool,
    quiet: bool,
) -> i32 {
    let mut tuning = DriftTuning::default();
    if let Some(path) = weights {
        match DriftTuning::from_toml_file(std::path::Path::new(&path)) {
            Ok(t) => tuning = t,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    }
    if let Some(cuts) = band_cutoffs {
        let parts: Vec<&str> = cuts.split(',').map(str::trim).collect();
        if parts.len() != 2 {
            eprintln!("error: --band-cutoffs expects \"high,low\" e.g. \"75,25\"");
            return 2;
        }
        match (parts[0].parse::<f64>(), parts[1].parse::<f64>()) {
            (Ok(h), Ok(l)) => {
                tuning.banding.high_absolute = h;
                tuning.banding.low_absolute = l;
                tuning.banding.small_list_threshold = usize::MAX; // force absolute
            }
            _ => {
                eprintln!("error: --band-cutoffs expects numeric values");
                return 2;
            }
        }
    }
    if let Some(cap) = cost_cap {
        tuning.reasoning.cost_cap_usd = cap;
    }

    let api_key = if llm {
        match std::env::var("OPENROUTER_API_KEY") {
            Ok(k) if !k.is_empty() => Some(k),
            _ => {
                eprintln!("error: --llm requires OPENROUTER_API_KEY in the environment");
                return 2;
            }
        }
    } else {
        None
    };

    let rules = driftwood_core::load_rules(None).unwrap_or_default();

    let config = ScanConfig {
        scopes: scope_categories(scope),
        privacy_tier: parse_privacy(privacy),
        stage2: llm,
        model: model.unwrap_or_else(|| driftwood_core::default_model().to_string()),
        api_key,
        allow_non_zdr: false,
        rules,
        tuning,
        persist: true,
    };

    let sink: Arc<dyn EventSink> = Arc::new(CliSink { quiet });
    let handle = ScanHandle::new();
    // Ctrl-C → prompt cancel (checked between phases/batches).
    let _ctrlc = {
        let h = handle.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\ncancelling (finishes current batch)…");
                h.cancel();
            }
        })
    };

    match run_scan(config, sink, handle).await {
        Ok(report) => {
            if dump_components {
                dump_component_table(&report);
            }
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn dump_component_table(report: &driftwood_core::Report) {
    use std::io::Write;
    let mut out = std::io::stderr().lock();
    let _ = writeln!(
        out,
        "{:<44} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>3}",
        "path",
        "score",
        "size",
        "age",
        "cache",
        "orph",
        "depth",
        "ftype",
        "kids",
        "total",
        "band"
    );
    for e in &report.entries {
        let c = &e.candidate;
        let s = c.score_components;
        let _ = writeln!(
            out,
            "{:<44} {:>5.1} {:>5.1} {:>5.1} {:>5.1} {:>5.1} {:>5.1} {:>5.1} {:>5.1} {:>5.1} {:>3?}",
            truncate_path(&c.path, 42),
            c.score,
            s.size,
            s.age,
            s.cache_location,
            s.orphan,
            s.depth,
            s.file_type,
            s.child_count,
            s.total(),
            format!("{:?}", c.band),
        );
    }
}

fn truncate_path(p: &str, max: usize) -> String {
    if p.len() <= max {
        p.to_string()
    } else {
        let keep = max - 1;
        format!("…{}", &p[p.len() - keep..])
    }
}

fn cmd_diff(a: &str, b: &str) -> i32 {
    let load = |p: &str| -> i32 {
        match std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<driftwood_core::Report>(&t).map_err(|e| e.to_string()))
        {
            Ok(r) => {
                let _ = r;
                0
            }
            Err(e) => {
                eprintln!("error reading {p}: {e}");
                2
            }
        }
    };
    if load(a) != 0 || load(b) != 0 {
        return 2;
    }
    let ra: driftwood_core::Report =
        serde_json::from_str(&std::fs::read_to_string(a).unwrap()).unwrap();
    let rb: driftwood_core::Report =
        serde_json::from_str(&std::fs::read_to_string(b).unwrap()).unwrap();

    let map = |r: &driftwood_core::Report| -> std::collections::HashMap<String, (u8, f64)> {
        r.entries
            .iter()
            .map(|e| {
                (
                    e.candidate.path.clone(),
                    (e.tier as u8, e.candidate.score),
                )
            })
            .collect()
    };
    let (ma, mb) = (map(&ra), map(&rb));

    let mut appeared = 0;
    let mut vanished = 0;
    let mut changed = 0;
    for (path, (tier_a, score_a)) in &ma {
        match mb.get(path) {
            None => {
                appeared += 1;
                println!("- {path} (tier {tier_a}, score {score_a:.1}) gone in B");
            }
            Some((tier_b, score_b)) => {
                if tier_a != tier_b {
                    changed += 1;
                    println!("~ {path}: tier {tier_a} → {tier_b} (score {score_a:.1} → {score_b:.1})");
                }
            }
        }
    }
    for (path, (tier_b, score_b)) in &mb {
        if !ma.contains_key(path) {
            vanished += 1;
            println!("+ {path} (tier {tier_b}, score {score_b:.1}) new in B");
        }
    }
    println!(
        "\n{} changed, {} gone in B, {} new in B",
        changed, appeared, vanished
    );
    0
}

async fn cmd_correct(path: &str, from: u8, to: u8, note: Option<String>, size: Option<u64>) -> i32 {
    let (Ok(from_t), Ok(to_t)) = (Tier::try_from(from), Tier::try_from(to)) else {
        eprintln!("error: tiers must be 1..4");
        return 2;
    };
    let req = CorrectionRequest {
        path: path.to_string(),
        size_bytes: size.unwrap_or(0),
        orphan: OrphanStatus::Unknown,
        score_band: driftwood_core::Band::Middle,
        original_tier: from_t,
        corrected_tier: to_t,
        note,
    };
    match apply_correction(req, None).await {
        Ok(rules) => {
            println!("correction recorded; {} rules now in rules.json", rules.len());
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn cmd_rules() -> i32 {
    match driftwood_core::load_rules(None) {
        Ok(rules) => {
            println!("{}", serde_json::to_string_pretty(&rules).unwrap());
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn cmd_tuning() -> i32 {
    println!("{}", DriftTuning::default().to_toml_string());
    0
}
