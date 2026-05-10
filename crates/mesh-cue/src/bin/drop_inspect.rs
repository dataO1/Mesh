//! A5 drop-detection spike: per-track energy-peak window CENTRES across the
//! whole local collection. Run-once diagnostic — no DB writes.
//!
//! For each track:
//!   1. Decode → mono mix (matches `reanalyze_ml.rs` code path)
//!   2. Compute mel spectrogram
//!   3. Compute per-frame energy envelope (mel-band-sum + bass boost)
//!   4. Pick top-2 non-overlapping 30 s windows by rolling-sum energy
//!   5. Report each window's CENTRE (start + window_frames / 2) as the
//!      candidate drop timestamp for ear-spot-check
//!
//! Usage:
//!   cargo run -p mesh-cue --release --bin drop_inspect -- \
//!     --collection ~/Music/mesh-collection \
//!     --out "/path/to/Mesh — A5 Drop Detection Spike.md"
//!     [--limit N]

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Context;
use rayon::prelude::*;

use mesh_core::audio_file::AudioFileReader;
use mesh_core::db::DatabaseService;
use mesh_cue::ml_analysis::{
    inference::{
        compute_energy_envelope, select_energy_peak_windows,
        ENERGY_BASS_BANDS, ENERGY_BASS_BOOST, ENERGY_PEAK_WINDOW_FRAMES,
        ENERGY_PEAK_WINDOWS,
    },
    preprocessing::{compute_mel_spectrogram, MUQ_HOP, MUQ_TARGET_SR},
};

struct Args {
    collection: PathBuf,
    out: PathBuf,
    limit: Option<usize>,
}

fn parse_args() -> Args {
    let mut collection: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut limit: Option<usize> = None;
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--collection" | "-c" => { collection = raw.get(i+1).map(PathBuf::from); i += 2; }
            "--out" | "-o" => { out = raw.get(i+1).map(PathBuf::from); i += 2; }
            "--limit" | "-l" => { limit = raw.get(i+1).and_then(|s| s.parse().ok()); i += 2; }
            "--help" | "-h" => {
                eprintln!("usage: drop_inspect --out <path.md> [--collection <path>] [--limit N]");
                std::process::exit(0);
            }
            other => { eprintln!("drop_inspect: unknown arg '{}'", other); std::process::exit(2); }
        }
    }
    Args {
        collection: collection.unwrap_or_else(|| {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
                .join("Music").join("mesh-collection")
        }),
        out: out.expect("--out is required"),
        limit,
    }
}

fn create_mono_mix(stems: &mesh_core::audio_file::StemBuffers) -> Vec<f32> {
    let len = stems.len();
    let mut mono = Vec::with_capacity(len);
    for i in 0..len {
        let v = (stems.vocals[i].left + stems.vocals[i].right) * 0.5;
        let d = (stems.drums[i].left + stems.drums[i].right) * 0.5;
        let b = (stems.bass[i].left + stems.bass[i].right) * 0.5;
        let o = (stems.other[i].left + stems.other[i].right) * 0.5;
        mono.push(v + d + b + o);
    }
    mono
}

struct DropRow {
    track_id: i64,
    artist: String,
    title: String,
    bpm: Option<f64>,
    duration_s: f64,
    drop_a_s: Option<f64>,
    drop_b_s: Option<f64>,
    note: String,
}

fn fmt_mmss(s: f64) -> String {
    let total = s.max(0.0) as i64;
    let m = total / 60;
    let sec = total % 60;
    format!("{:02}:{:02}", m, sec)
}

fn esc_md(s: &str) -> String {
    s.replace('|', r"\|").replace('\n', " ").replace('\r', " ")
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = parse_args();

    eprintln!("[drop_inspect] DB at {}/mesh.db", args.collection.display());
    let db = DatabaseService::new(&args.collection)
        .map_err(|e| anyhow::anyhow!("DB open: {}", e))?;

    let tracks = db.get_all_tracks()
        .map_err(|e| anyhow::anyhow!("get_all_tracks: {}", e))?;
    let mut work: Vec<_> = tracks.into_iter().filter(|t| t.id.is_some()).collect();
    if let Some(n) = args.limit { work.truncate(n); }
    let total = work.len();

    let frame_to_seconds = MUQ_HOP as f64 / MUQ_TARGET_SR as f64;
    let window_seconds = ENERGY_PEAK_WINDOW_FRAMES as f64 * frame_to_seconds;
    let window_centre_offset_s = window_seconds / 2.0;
    eprintln!(
        "[drop_inspect] {} tracks; window={:.0}s, n_windows={}, bass_boost={}× on bands 0..{}",
        total, window_seconds, ENERGY_PEAK_WINDOWS, ENERGY_BASS_BOOST, ENERGY_BASS_BANDS,
    );

    let rows: Mutex<Vec<DropRow>> = Mutex::new(Vec::with_capacity(total));
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let start = std::time::Instant::now();

    work.par_iter().for_each(|track| {
        let track_id = track.id.unwrap();
        let path = track.path.clone();
        let result: anyhow::Result<(Option<f64>, Option<f64>, String)> = (|| {
            let reader = AudioFileReader::open(&path)
                .with_context(|| format!("open {}", path.display()))?;
            let stems = reader.read_all_stems()
                .with_context(|| "read_all_stems".to_string())?;
            let mono = create_mono_mix(&stems);
            drop(stems);
            let mel = compute_mel_spectrogram(&mono, mesh_core::types::SAMPLE_RATE as f32)
                .map_err(|e| anyhow::anyhow!("mel: {}", e))?;
            drop(mono);

            if mel.frames.len() < ENERGY_PEAK_WINDOWS * ENERGY_PEAK_WINDOW_FRAMES {
                return Ok((None, None, format!("too short ({} mel frames)", mel.frames.len())));
            }

            let energy = compute_energy_envelope(&mel.frames, true);
            let starts = select_energy_peak_windows(
                &energy, ENERGY_PEAK_WINDOW_FRAMES, ENERGY_PEAK_WINDOWS,
            );
            let centres: Vec<f64> = starts.iter()
                .map(|&s| s as f64 * frame_to_seconds + window_centre_offset_s)
                .collect();
            let (a, b) = match centres.as_slice() {
                [x, y] => (Some(*x), Some(*y)),
                [x] => (Some(*x), None),
                _ => (None, None),
            };
            Ok((a, b, String::new()))
        })();

        let (drop_a_s, drop_b_s, note) = match result {
            Ok(v) => v,
            Err(e) => {
                failed.fetch_add(1, Ordering::Relaxed);
                (None, None, format!("ERROR: {}", e))
            }
        };

        rows.lock().unwrap().push(DropRow {
            track_id,
            artist: track.artist.clone().unwrap_or_default(),
            title: track.title.clone(),
            bpm: track.bpm,
            duration_s: track.duration_seconds,
            drop_a_s,
            drop_b_s,
            note,
        });

        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
        if n % 25 == 0 || n == total {
            let elapsed = start.elapsed().as_secs_f32();
            let rate = n as f32 / elapsed.max(0.001);
            let eta = (total - n) as f32 / rate.max(0.001);
            eprintln!(
                "[drop_inspect] {}/{} done ({:.1}/s, eta {:.0}s, failed {})",
                n, total, rate, eta, failed.load(Ordering::Relaxed),
            );
        }
    });

    let mut rows = rows.into_inner().unwrap();
    rows.sort_by(|a, b| {
        a.artist.to_lowercase().cmp(&b.artist.to_lowercase())
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });

    use std::fmt::Write;
    let mut md = String::new();
    writeln!(md, "---").unwrap();
    writeln!(md, "tags: [knowledge-base, mesh, intensity-axis, a5, drop-detection-spike]").unwrap();
    writeln!(md, "library: {}", args.collection.display()).unwrap();
    writeln!(md, "n_tracks: {}", rows.len()).unwrap();
    writeln!(md, "energy_source: mel-band-sum (shifted-positive dB), bass boost {}× on bands 0..{}",
        ENERGY_BASS_BOOST, ENERGY_BASS_BANDS).unwrap();
    writeln!(md, "window_seconds: {:.0}", window_seconds).unwrap();
    writeln!(md, "n_windows: {}", ENERGY_PEAK_WINDOWS).unwrap();
    writeln!(md, "status: run-once spike, no DB writes").unwrap();
    writeln!(md, "---\n").unwrap();
    writeln!(md, "# A5 energy-peak window detection — library spike\n").unwrap();
    writeln!(md, "**Drop A** and **Drop B** are the *centres* of the two highest-energy non-overlapping {:.0} s windows that A5 picks per track — i.e. what A5 would feed into V18.X for intensity scoring. **NOT** drop *transitions* (bass-reintroduction events); these are energy *plateaus*.\n", window_seconds).unwrap();
    writeln!(md, "Spot-check method: open each track at the listed timestamp ± 15 s in any audio player, judge whether the window covers a section a DJ would call \"the peak / drop\" of the track.\n").unwrap();
    writeln!(md, "Sorted by artist + title.\n").unwrap();

    writeln!(md, "| # | track_id | artist | title | bpm | duration | drop A | drop B | gap (s) | note |").unwrap();
    writeln!(md, "|---:|---:|---|---|---:|---:|---:|---:|---:|---|").unwrap();
    for (i, r) in rows.iter().enumerate() {
        let bpm = r.bpm.map(|b| format!("{:.0}", b)).unwrap_or_default();
        let dur = fmt_mmss(r.duration_s);
        let dropa = r.drop_a_s.map(fmt_mmss).unwrap_or_else(|| "—".into());
        let dropb = r.drop_b_s.map(fmt_mmss).unwrap_or_else(|| "—".into());
        let gap = match (r.drop_a_s, r.drop_b_s) {
            (Some(a), Some(b)) => format!("{:.0}", (b - a).abs()),
            _ => "—".into(),
        };
        writeln!(md, "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            i + 1, r.track_id, esc_md(&r.artist), esc_md(&r.title),
            bpm, dur, dropa, dropb, gap, esc_md(&r.note),
        ).unwrap();
    }

    std::fs::write(&args.out, md)?;

    let elapsed = start.elapsed().as_secs_f32();
    eprintln!(
        "[drop_inspect] wrote {} ({} rows, {} failed, {:.0}s, {:.2}/s)",
        args.out.display(), rows.len(), failed.load(Ordering::Relaxed),
        elapsed, total as f32 / elapsed.max(0.001),
    );
    Ok(())
}
