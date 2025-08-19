use anyhow::{anyhow, Context, Result};
use clap::{ArgAction, Parser, ValueEnum, ValueHint};
use img_hash::HasherConfig;
use rayon::prelude::*;
use slidesplit::{cluster_frames, merge_short_clusters, FrameEntry};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use tracing::{debug, error, info, instrument, warn};
use walkdir::WalkDir;

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

/// Centralized configuration for slidesplit operations
#[derive(Debug, Clone)]
pub struct Config {
    /// Input video file path
    pub input: PathBuf,
    /// Output directory path
    pub out_dir: PathBuf,
    /// Sampling frames per second before de-duplication
    pub fps: f32,
    /// Hamming distance threshold (0..=64) to separate slides
    pub threshold: u32,
    /// Minimum stable duration in seconds to accept a slide
    pub min_stable_seconds: f32,
    /// Keep temporary extracted frames
    pub keep_temps: bool,
    /// Output format
    pub format: OutFormat,
    /// For WEBP only: use lossless mode
    pub webp_lossless: bool,
    /// FFmpeg binary path
    pub ffmpeg_bin: PathBuf,
}

impl Config {
    /// Create config from CLI args, with validation and defaults applied
    #[instrument(name = "config_from_args")]
    pub fn from_args(args: Args) -> Result<Self> {
        // Validate input file exists
        if !args.input.exists() {
            return Err(anyhow!("Input file not found: {}", args.input.display()));
        }

        // Determine output directory
        let out_dir = args
            .out_dir
            .unwrap_or_else(|| default_out_dir(&args.input));

        // Validate parameters
        if args.fps <= 0.0 {
            return Err(anyhow!("FPS must be positive, got: {}", args.fps));
        }
        if args.threshold > 64 {
            return Err(anyhow!("Threshold must be 0..=64, got: {}", args.threshold));
        }
        if args.min_stable_seconds < 0.0 {
            return Err(anyhow!("min_stable_seconds must be non-negative, got: {}", args.min_stable_seconds));
        }

        // Warn about lossy formats
        if matches!(args.format, OutFormat::Jpg | OutFormat::Jpeg) {
            warn!("JPEG is not lossless. Consider --format png/webp --webp-lossless/tiff/bmp for lossless output.");
        }

        // Get ffmpeg binary
        let ffmpeg_bin = ensure_ffmpeg_available()?;

        info!("Configuration initialized");
        debug!("Config: input={}, out_dir={}, fps={}, threshold={}", 
               args.input.display(), out_dir.display(), args.fps, args.threshold);

        Ok(Config {
            input: args.input,
            out_dir,
            fps: args.fps,
            threshold: args.threshold,
            min_stable_seconds: args.min_stable_seconds,
            keep_temps: args.keep_temps,
            format: args.format,
            webp_lossless: args.webp_lossless,
            ffmpeg_bin,
        })
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

    /// Set logging level: error, warn, info, debug, trace
    #[arg(short, long, default_value = "info")]
    verbosity: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    // Initialize structured logging
    init_logging(&args.verbosity)?;
    
    info!("Starting slidesplit v{}", env!("CARGO_PKG_VERSION"));

    // Create configuration with validation
    let config = Config::from_args(args)?;

    // Run the main processing pipeline
    process_video(config)?;

    info!("Processing completed successfully");
    Ok(())
}

/// Initialize structured logging based on verbosity level
fn init_logging(verbosity: &str) -> Result<()> {
    let level = match verbosity.to_lowercase().as_str() {
        "error" => "error",
        "warn" => "warn", 
        "info" => "info",
        "debug" => "debug",
        "trace" => "trace",
        _ => return Err(anyhow!("Invalid verbosity level: {}. Use: error, warn, info, debug, trace", verbosity)),
    };

    tracing_subscriber::fmt()
        .with_env_filter(format!("slidesplit={}", level))
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .init();

    debug!("Logging initialized at level: {}", level);
    Ok(())
}

/// Main video processing pipeline
#[instrument(name = "process_video", skip(config))]
fn process_video(config: Config) -> Result<()> {
    info!("Creating output directory: {}", config.out_dir.display());
    fs::create_dir_all(&config.out_dir)
        .with_context(|| format!("Failed to create output directory: {}", config.out_dir.display()))?;

    let frames_dir = TempDir::new().context("Failed to create temporary directory for frames")?;
    debug!("Created temporary directory: {}", frames_dir.path().display());

    // Extract frames
    extract_frames(&config, frames_dir.path())?;

    // Load and hash frames
    let frames = load_frame_hashes(frames_dir.path())?;
    if frames.is_empty() {
        return Err(anyhow!("No frames extracted. Is the video valid?"));
    }
    info!("Loaded {} frames for processing", frames.len());

    // Cluster frames
    let mut clusters = cluster_frames(&frames, config.threshold);
    info!("Initial clustering produced {} clusters", clusters.len());
    
    merge_short_clusters(
        &mut clusters,
        &frames,
        config.min_stable_seconds,
        config.fps,
        config.threshold,
    );
    info!("After merging short clusters: {} final clusters", clusters.len());

    // Write output slides
    let wrote = write_output_slides(&config, &clusters, &frames)?;

    // Optionally keep temporary frames
    if config.keep_temps {
        keep_temporary_frames(&config, frames_dir.path())?;
    }

    info!("Done. Wrote {} slide{} to {}", 
          wrote, 
          if wrote == 1 { "" } else { "s" }, 
          config.out_dir.display());
    Ok(())
}

/// Write representative frames for each cluster to output directory
#[instrument(name = "write_output", skip(config, clusters, frames))]
fn write_output_slides(config: &Config, clusters: &[Vec<usize>], frames: &[FrameEntry]) -> Result<usize> {
    let ext = config.format.ext();
    debug!("Writing output slides in format: {}", ext);

    let wrote: usize = clusters
        .par_iter()
        .enumerate()
        .map(|(slide_num, cluster)| -> Result<usize> {
            if cluster.is_empty() {
                debug!("Skipping empty cluster {}", slide_num);
                return Ok(0);
            }
            
            // Use median frame of cluster as representative
            let rep = &frames[cluster[cluster.len() / 2]];
            let out_name = format!("slide_{:02}.{}", slide_num, ext);
            let out_path = config.out_dir.join(&out_name);
            
            debug!("Writing slide {} from frame {} to {}", slide_num, rep.idx, out_name);
            
            fs::copy(&rep.path, &out_path).with_context(|| {
                format!(
                    "Failed to copy representative frame {} -> {}",
                    rep.path.display(),
                    out_path.display()
                )
            })?;
            Ok(1)
        })
        .try_reduce(|| 0, |a, b| Ok(a + b))?;

    if wrote == 0 {
        return Err(anyhow!(
            "No slides detected (threshold too strict?). Try lowering --threshold or increasing --fps."
        ));
    }

    Ok(wrote)
}

/// Keep temporary frames in output directory if requested
#[instrument(name = "keep_temps", skip(config))]
fn keep_temporary_frames(config: &Config, frames_dir: &Path) -> Result<()> {
    let keep_path = config.out_dir.join("frames_raw");
    info!("Keeping temporary frames in: {}", keep_path.display());
    
    fs::create_dir_all(&keep_path)
        .with_context(|| format!("Failed to create frames_raw directory: {}", keep_path.display()))?;
    
    let mut copied = 0;
    for entry in WalkDir::new(frames_dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let source_path = entry.path();
        if source_path.is_file() && source_path.extension().is_some() {
            if let Some(filename) = source_path.file_name() {
                let dest_path = keep_path.join(filename);
                fs::copy(source_path, &dest_path)
                    .with_context(|| format!("Failed to copy temp frame: {} -> {}", 
                                            source_path.display(), dest_path.display()))?;
                copied += 1;
            }
        }
    }
    
    debug!("Copied {} temporary frames to frames_raw", copied);
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
#[instrument(name = "ensure_ffmpeg")]
fn ensure_ffmpeg_available() -> Result<PathBuf> {
    // Try system ffmpeg first
    debug!("Checking for system ffmpeg");
    let ok = Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    
    if ok {
        info!("Using system ffmpeg");
        Ok(PathBuf::from("ffmpeg"))
    } else {
        info!("System ffmpeg not found, downloading sidecar binary");
        let target_dir = ffmpeg_sidecar::paths::sidecar_dir()
            .context("Failed to determine sidecar directory")?;
        debug!("Downloading ffmpeg to: {}", target_dir.display());
        
        ffmpeg_sidecar::download::auto_download()
            .context("Failed to download ffmpeg sidecar")?;
            
        info!("Successfully downloaded ffmpeg sidecar");
        Ok(target_dir)
    }
}

#[instrument(name = "run_command", skip(cmd))]
fn run_and_stream(cmd: &mut Command) -> Result<std::process::ExitStatus> {
    // Log the command being executed (at debug level to avoid leaking sensitive paths)
    debug!("Executing command: {}", format_command(cmd));
    
    // Inherit parent's stdout/stderr so the child output is streamed directly
    // to the console in real time without buffering here.
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let mut child = cmd.spawn().context("Failed to spawn command")?;
    let status = child.wait().context("Failed waiting for command")?;
    Ok(status)
}

/// Format a command for logging (hide sensitive path details)
fn format_command(cmd: &Command) -> String {
    format!("{:?}", cmd)
        .chars()
        .take(200) // Limit length to avoid huge logs
        .collect::<String>()
        + "..."
}

#[instrument(name = "extract_frames", skip(config))]
fn extract_frames(config: &Config, outdir: &Path) -> Result<()> {
    info!("Extracting frames at {} fps to {}", config.fps, outdir.display());
    
    fs::create_dir_all(outdir)
        .with_context(|| format!("Failed to create frames directory: {}", outdir.display()))?;

    let pattern = outdir.join(format!("frame_%06d.{}", config.format.ext()));
    let input_str = config.input.to_str()
        .ok_or_else(|| anyhow!("Input path contains invalid UTF-8: {}", config.input.display()))?;
    let pattern_str = pattern.to_str()
        .ok_or_else(|| anyhow!("Output pattern contains invalid UTF-8: {}", pattern.display()))?;

    let mut cmd = Command::new(&config.ffmpeg_bin);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        input_str,
        "-vf",
        &format!("fps={}", config.fps),
        "-vsync",
        "vfr",
    ]);

    // Format-specific lossless flags (encoder opts)
    match config.format {
        OutFormat::Webp if config.webp_lossless => {
            debug!("Using WebP lossless encoding");
            cmd.args(["-lossless", "1"]);
        }
        OutFormat::Tiff => {
            debug!("Using TIFF with LZW compression");
            cmd.args(["-compression_algo", "lzw"]);
        }
        OutFormat::Bmp => {
            debug!("Using BMP format (inherently lossless)");
        }
        OutFormat::Png => {
            debug!("Using PNG with high compression");
            cmd.args(["-compression_level", "12"]);
        }
        OutFormat::Jpg | OutFormat::Jpeg => {
            debug!("Using JPEG with high quality (not lossless)");
            cmd.args(["-qscale:v", "2"]);
        }
        _ => {}
    }

    cmd.arg(pattern_str);

    debug!("Starting frame extraction");
    let status = run_and_stream(&mut cmd)?;
    
    if !status.success() {
        return Err(anyhow!("ffmpeg failed to extract frames (exit code: {:?})", status.code()));
    }
    
    info!("Frame extraction completed successfully");
    Ok(())
}

#[instrument(name = "load_hashes", skip(dir))]
fn load_frame_hashes(dir: &Path) -> Result<Vec<FrameEntry>> {
    debug!("Loading frame hashes from: {}", dir.display());

    // Collect and sort paths by numeric index (…_%06d.ext)
    let mut entries: Vec<(usize, PathBuf)> = WalkDir::new(dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| match e {
            Ok(entry) => Some(entry),
            Err(err) => {
                warn!("Error walking directory: {}", err);
                None
            }
        })
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

    if entries.is_empty() {
        return Err(anyhow!("No frame files found in directory: {}", dir.display()));
    }

    entries.par_sort_by_key(|(i, _)| *i);
    info!("Found {} frame files to process", entries.len());

    // Parallel load + hash with better error handling
    // Create a separate hasher for each thread to avoid Send/Sync issues
    let results: Vec<Result<FrameEntry>> = entries
        .par_iter()
        .map(|(idx, path)| -> Result<FrameEntry> {
            // DCT 8x8 = 64-bit perceptual hash (create per-thread to avoid sync issues)
            let hasher = HasherConfig::new().hash_size(8, 8).to_hasher();
            
            let dynimg = image::open(path)
                .with_context(|| format!("Failed to open frame image: {}", path.display()))?;
            let rgba = dynimg.to_rgba8();
            let (w, h) = rgba.dimensions();
            let raw = rgba.into_raw();
            
            let buf = img_hash::image::ImageBuffer::<img_hash::image::Rgba<u8>, Vec<u8>>::from_raw(w, h, raw)
                .ok_or_else(|| anyhow!("Failed to build image buffer for hashing: {}", path.display()))?;
            let hash = hasher.hash_image(&buf);
            
            Ok(FrameEntry {
                idx: *idx,
                path: path.clone(),
                hash,
            })
        })
        .collect();

    // Separate successful results from errors
    let mut successful = Vec::new();
    let mut error_count = 0;
    
    for result in results {
        match result {
            Ok(frame_entry) => successful.push(frame_entry),
            Err(e) => {
                error!("Failed to process frame: {:#}", e);
                error_count += 1;
            }
        }
    }

    if error_count > 0 {
        warn!("Failed to process {} out of {} frames", error_count, entries.len());
    }

    if successful.is_empty() {
        return Err(anyhow!("No frames could be processed successfully"));
    }

    info!("Successfully loaded and hashed {} frames", successful.len());
    Ok(successful)
}
