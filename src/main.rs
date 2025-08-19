use anyhow::{anyhow, Context, Result};
use clap::{ArgAction, Parser, ValueEnum, ValueHint};
use img_hash::{HasherConfig, ImageHash};
use rayon::prelude::*;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use walkdir::WalkDir;

// near the top
#[cfg(test)]
pub(crate) use crate::{cluster_frames, merge_short_clusters};

/// Output image formats (note: jpg/jpeg are NOT lossless).
#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutFormat {
    Png,
    Webp,
    Tiff,
    Bmp,
    Jpg,
    Jpeg,
}

impl OutFormat {
    fn ext(self) -> &'static str {
        match self {
            OutFormat::Png => "png",
            OutFormat::Webp => "webp",
            OutFormat::Tiff => "tiff",
            OutFormat::Bmp => "bmp",
            OutFormat::Jpg | OutFormat::Jpeg => "jpg",
        }
    }
    fn is_lossless_default(self) -> bool {
        matches!(self, OutFormat::Png | OutFormat::Tiff | OutFormat::Bmp)
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Input video file (e.g., slideshow.mp4)
    #[arg(value_hint = ValueHint::FilePath)]
    input: PathBuf,

    /// Output directory (created if missing). Defaults to "<input_stem>_slides"
    #[arg(short, long, value_hint = ValueHint::DirPath)]
    out_dir: Option<PathBuf>,

    /// Sampling frames per second before de-duplication
    #[arg(long, default_value_t = 2.0)]
    fps: f32,

    /// Hamming distance threshold (0..=64) to separate slides
    #[arg(long, default_value_t = 10)]
    threshold: u32,

    /// Minimum stable duration in seconds to accept a slide (merges transition blobs)
    #[arg(long, default_value_t = 1.0)]
    min_stable_seconds: f32,

    /// Keep temporary extracted frames
    #[arg(long, action = ArgAction::SetTrue)]
    keep_temps: bool,

    /// Output format: png, webp, tiff, bmp, jpg, jpeg
    #[arg(long, value_enum, default_value_t = OutFormat::Png)]
    format: OutFormat,

    /// For WEBP only: make the encoder use lossless mode
    #[arg(long, action = ArgAction::SetTrue)]
    webp_lossless: bool,
}

#[derive(Clone)]
struct FrameEntry {
    idx: usize,
    path: PathBuf,
    hash: ImageHash,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let ffmpeg = ensure_ffmpeg_available()?;

    if !args.input.exists() {
        return Err(anyhow!("Input file not found: {}", args.input.display()));
    }
    if matches!(args.format, OutFormat::Jpg | OutFormat::Jpeg) {
        eprintln!(
            "Warning: JPEG is not lossless. Use --format png/webp --webp-lossless/tiff/bmp for lossless output."
        );
    }

    let out_dir = args
        .out_dir
        .clone()
        .unwrap_or_else(|| default_out_dir(&args.input));
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("Creating output dir {}", out_dir.display()))?;

    let frames_dir = TempDir::new().context("Creating temp dir for frames")?;

    // Extract frames directly in the requested format (saves re-encoding later).
    extract_frames(
        &ffmpeg,
        &args.input,
        frames_dir.path(),
        args.fps,
        args.format,
        args.webp_lossless,
    )?;

    let frames = load_frame_hashes(frames_dir.path())?;
    if frames.is_empty() {
        return Err(anyhow!("No frames extracted. Is the video valid?"));
    }

    let mut clusters = cluster_frames(&frames, args.threshold);
    merge_short_clusters(
        &mut clusters,
        &frames,
        args.min_stable_seconds,
        args.fps,
        args.threshold,
    );

    // Parallel write: one representative per cluster
    let ext = args.format.ext().to_string();
    let wrote: usize = clusters
        .par_iter()
        .enumerate()
        .map(|(slide_num, cl)| -> Result<()> {
            if cl.is_empty() {
                return Ok(());
            }
            // median frame of cluster
            let rep = &frames[cl[cl.len() / 2]];
            let out_name = format!("slide_{:02}.{}", slide_num, ext);
            let out_path = out_dir.join(out_name);
            fs::copy(&rep.path, &out_path).with_context(|| {
                format!(
                    "Copying representative frame {} -> {}",
                    rep.path.display(),
                    out_path.display()
                )
            })?;
            Ok(())
        })
        .filter_map(|r| match r {
            Ok(_) => Some(1),
            Err(e) => {
                eprintln!("Write error: {e:#}");
                None
            }
        })
        .sum();

    if wrote == 0 {
        return Err(anyhow!("No slides detected (threshold too strict?). Try lowering --threshold or increasing --fps."));
    }

    if args.keep_temps {
        let keep_path = out_dir.join("frames_raw");
        fs::create_dir_all(&keep_path)?;
        for entry in WalkDir::new(frames_dir.path())
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .par_bridge()
        {
            let p = entry.path().to_path_buf();
            if p.extension().and_then(OsStr::to_str).is_some() {
                let fname = p.file_name().unwrap().to_owned();
                let dest = keep_path.join(fname);
                let _ = fs::copy(p, dest);
            }
        }
    }

    println!(
        "Done. Wrote {} slide{} to {}",
        wrote,
        if wrote == 1 { "" } else { "s" },
        out_dir.display()
    );
    Ok(())
}

fn default_out_dir(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("output");
    PathBuf::from(format!("{}_slides", stem))
}

/// Returns a path to an ffmpeg executable. If system ffmpeg is missing,
/// downloads a static sidecar binary for this platform.
fn ensure_ffmpeg_available() -> Result<PathBuf> {
    // Try system ffmpeg first
    let ok = Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Ok(PathBuf::from("ffmpeg"))
    } else {
        ffmpeg_sidecar::download::auto_download()?;
        ffmpeg_sidecar::paths::sidecar_dir()
    }
}

fn extract_frames(
    ffmpeg_bin: &Path,
    input: &Path,
    outdir: &Path,
    fps: f32,
    fmt: OutFormat,
    webp_lossless: bool,
) -> Result<()> {
    fs::create_dir_all(outdir)?;

    let pattern = outdir.join(format!("frame_%06d.{}", fmt.ext()));
    let mut cmd = Command::new(ffmpeg_bin);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        input.to_str().unwrap(),
        "-vf",
        &format!("fps={}", fps),
        "-vsync",
        "vfr",
    ]);

    // Format-specific lossless flags (encoder opts)
    match fmt {
        OutFormat::Webp if webp_lossless => {
            // libwebp encoder supports -lossless 1
            cmd.args(["-lossless", "1"]);
        }
        OutFormat::Tiff => {
            // default is uncompressed; can be large but lossless
            // (could also specify -compression lzw for smaller files)
            cmd.args(["-compression_algo", "lzw"]);
        }
        OutFormat::Bmp => {
            // BMP is inherently lossless; no extra flags needed
        }
        OutFormat::Png => {
            // PNG lossless; optionally tune compression level
            cmd.args(["-compression_level", "12"]);
        }
        OutFormat::Jpg | OutFormat::Jpeg => {
            // Not lossless. Use high quality if user insists.
            cmd.args(["-qscale:v", "2"]);
        }
        _ => {}
    }

    cmd.arg(pattern.to_str().unwrap());

    let status = cmd.status().context("Running ffmpeg to extract frames")?;
    if !status.success() {
        return Err(anyhow!("ffmpeg failed to extract frames"));
    }
    Ok(())
}

fn load_frame_hashes(dir: &Path) -> Result<Vec<FrameEntry>> {
    // DCT 8x8 = 64-bit perceptual hash
    let hasher = HasherConfig::new().hash_size(8, 8).to_hasher();

    // Collect and sort paths by numeric index (…_%06d.ext)
    let mut entries: Vec<(usize, PathBuf)> = WalkDir::new(dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.into_path();
            if p.is_file() {
                let stem = p.file_stem()?.to_string_lossy();
                let idx = stem.rsplit_once('_')?.1.parse::<usize>().ok()?;
                Some((idx, p))
            } else {
                None
            }
        })
        .collect();

    entries.par_sort_by_key(|(i, _)| *i);

    // Parallel load + hash
    let out: Vec<FrameEntry> = entries
        .par_iter()
        .map(|(idx, path)| -> Result<FrameEntry> {
            let img = image::open(path)
                .with_context(|| format!("Opening extracted frame {}", path.display()))?;
            let hash = hasher.hash_image(&img);
            Ok(FrameEntry {
                idx: *idx,
                path: path.clone(),
                hash,
            })
        })
        .filter_map(|r| match r {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("Hash error: {e:#}");
                None
            }
        })
        .collect();

    Ok(out)
}

/// Initial clustering: anchor strategy
pub(crate) fn cluster_frames(frames: &[FrameEntry], threshold: u32) -> Vec<Vec<usize>> {
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    if frames.is_empty() {
        return clusters;
    }
    let mut cur: Vec<usize> = vec![0];
    let mut anchor = &frames[0].hash;

    for i in 1..frames.len() {
        let d = frames[i].hash.dist(anchor);
        if d <= threshold {
            cur.push(i);
        } else {
            clusters.push(cur);
            cur = vec![i];
            anchor = &frames[i].hash;
        }
    }
    clusters.push(cur);
    clusters
}

/// Merge clusters shorter than min_stable_seconds into neighbors to avoid counting transition blobs.
pub(crate) fn merge_short_clusters(
    clusters: &mut Vec<Vec<usize>>,
    frames: &[FrameEntry],
    min_stable_seconds: f32,
    fps: f32,
    threshold: u32,
) {
    let min_len = (min_stable_seconds * fps).ceil() as usize;

    loop {
        let mut changed = false;
        let mut i = 0;
        while i < clusters.len() {
            if clusters[i].len() < min_len {
                let merge_target = if clusters.len() == 1 {
                    None
                } else if i == 0 {
                    Some(1usize)
                } else if i == clusters.len() - 1 {
                    Some(i - 1)
                } else {
                    let cur_first = &frames[clusters[i][0]].hash;
                    let cur_last = &frames[*clusters[i].last().unwrap()].hash;

                    let prev_last = &frames[*clusters[i - 1].last().unwrap()].hash;
                    let next_first = &frames[clusters[i + 1][0]].hash;

                    let d_prev = cur_first.dist(prev_last);
                    let d_next = cur_last.dist(next_first);
                    if d_prev <= d_next {
                        Some(i - 1)
                    } else {
                        Some(i + 1)
                    }
                };

                if let Some(t) = merge_target {
                    let mut take = clusters.remove(i);
                    if t < i {
                        clusters[t].append(&mut take);
                        i = t;
                    } else {
                        clusters[t - 1].append(&mut take);
                        i = t - 1;
                    }
                    changed = true;
                    continue;
                }
            }
            i += 1;
        }
        if !changed {
            break;
        }
    }

    // Merge micro-splits
    let mut i = 0;
    while i + 1 < clusters.len() {
        let a_last = frames[*clusters[i].last().unwrap()].hash.clone();
        let b_first = frames[clusters[i + 1][0]].hash.clone();
        if a_last.dist(&b_first) <= threshold / 2 {
            let mut tail = clusters.remove(i + 1);
            clusters[i].append(&mut tail);
        } else {
            i += 1;
        }
    }
}
