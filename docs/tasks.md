# Improvement Tasks for slidesplit

Below is an ordered, actionable checklist. Each item starts with a checkbox to track completion.

## Architecture & Core Implementation ✅ Completed

1. [x] Convert the project to a hybrid binary+library crate (expose core logic in lib.rs, keep CLI in main.rs).
2. [x] Extract frame clustering and merging logic into a separate module and expose a clean API (implemented in lib.rs).
3. [x] Extract ffmpeg detection/download and invocation into a dedicated module with a single entry point (implemented as `ensure_ffmpeg_available()`).
4. [x] **Introduce a Config struct to centralize parameters (fps, threshold, min_stable_seconds, format, webp_lossless, paths).**
5. [x] **Replace ad-hoc eprintln!/println! with a structured logging crate (env_logger or tracing) and user-selectable verbosity.**
6. [x] **Standardize error handling: eliminate remaining unwrap() calls on path conversions; add context() at I/O and process boundaries.**
7. [x] Validate CLI arguments more strictly (clap handles type validation; range checks implemented via ValueEnum).

## FFmpeg & Format Support ✅ Well Implemented

8. [x] Add an option to force using system ffmpeg vs sidecar and provide clear diagnostics when neither is available.
9. [ ] Add detection of ffprobe (if needed) and report codec/pixel format info for better troubleshooting at higher verbosity.
10. [x] Support multiple output formats (PNG, WebP, TIFF, BMP, JPG/JPEG) with format-specific lossless optimizations.
11. [x] WebP lossless support via `--webp-lossless` flag.

## Performance & Optimization 🔄 Partially Complete

12. [x] Guard parallel I/O operations using rayon for frame hashing and output writing.
13. [ ] Optimize hashing pipeline: decode frames at a smaller resolution for hashing only, but copy original frames for output.
14. [ ] Avoid repeated image decoding when only file copying is needed for representative frames (separate metadata vs data paths).
15. [ ] Make clustering strategy pluggable (anchor-based current approach vs sliding window vs DBSCAN variant) behind a trait.

## Testing ✅ Good Foundation

16. [x] Add unit tests for core clustering logic (`cluster_frames`, `merge_short_clusters`).
17. [x] Add integration tests for end-to-end pipeline with synthetic videos.
18. [ ] Add unit tests for edge cases: empty frames dir, single-frame input, all-identical frames, rapidly alternating frames.
19. [ ] Add property-based tests (proptest/quickcheck) for clustering invariants (e.g., partitions cover all indices, are ordered, non-overlapping).
20. [ ] Add integration test for all output formats (JPEG/WebP/TIFF/BMP) and webp_lossless flag behavior.
21. [ ] Add tests to confirm merge_short_clusters behavior around boundary lengths (exactly equal to min_len, threshold/2 micro-splits).
22. [ ] Add benchmarks (criterion) for load_frame_hashes, cluster_frames, and merge_short_clusters on synthetic datasets.

## User Experience & Features

23. [x] Keep temporary extracted frames option (`--keep-temps` flag implemented).
24. [ ] Add graceful cancellation handling (Ctrl-C) to clean up temp dirs and partially written outputs.
25. [ ] Allow specifying a fixed output filename pattern and zero-padding width; validate collisions in out_dir.
26. [x] Improve default output directory derivation (implemented as `<input_stem>_slides`).
27. [ ] Add progress reporting (per 100 frames hashed, per cluster written) at info/debug levels.
28. [ ] Emit a machine-readable summary (JSON) of detected slides: indices, time ranges, representative frame path.
29. [ ] Introduce a dry-run mode that performs analysis without writing image outputs (prints summary only).

## Advanced Features

30. [ ] Support reading from image sequences as input (glob pattern) in addition to video files.
31. [ ] Allow user-provided ROI/cropping to ignore borders/watermarks during hashing; add CLI options.
32. [ ] Add a configurable hash algorithm and parameters (DCT size, block size) via CLI and Config (currently fixed at 8x8 DCT).

## Documentation & Examples

33. [ ] Document the algorithm, CLI usage, and examples in README and docs/ (include performance tips and known limitations).
34. [ ] Improve help text and examples in clap, including recommended settings for cross-fades and hard cuts.
35. [ ] Provide a minimal library example in examples/ demonstrating use of the core API without the CLI.
36. [ ] Add code comments and docstrings for public APIs; generate docs with cargo doc and link from README.
37. [ ] Create a CONTRIBUTING.md with development workflow, testing instructions, style, and release process.

## CI/CD & Release Management

38. [ ] Set up CI (GitHub Actions) to run tests on Linux/macOS/Windows with a matrix including system ffmpeg present/absent.
39. [ ] Add cargo deny/audit to check for vulnerable or unmaintained dependencies in CI.
40. [ ] Add rustfmt and clippy checks in CI; fix all clippy warnings and apply rustfmt consistently.
41. [ ] Package releases: create release workflow to build static binaries and attach to GitHub Releases; include checksums.
42. [ ] Enable reproducible builds where possible and document how to verify them.

## Advanced Implementation Details

43. [ ] Introduce a logging guard around ffmpeg commands to print full command lines at debug level (without leaking sensitive paths).
44. [ ] Add retry/backoff for transient ffmpeg-sidecar download errors and cache location diagnostics.
45. [ ] Add a feature flag to disable ffmpeg-sidecar for distro packaging scenarios (pure system-ffmpeg use).
46. [ ] Add telemetry hooks (optional) or metrics counters (e.g., slides_detected, frames_hashed, io_errors) guarded by a feature.
47. [ ] Refactor file walking to be more robust against non-standard filenames and permission errors (skip with warnings).
48. [ ] Ensure temp directories are created under a user-configurable base dir and are always cleaned unless --keep-temps.
49. [ ] Add memory usage considerations: stream listing and hashing to avoid holding too many paths/hashes at once for huge inputs.
50. [ ] Consider exposing a WASM-friendly path by abstracting filesystem and process invocations behind traits (future-proofing).

## Current Status Summary - Updated August 2025

**✅ Completed in this session:**
- **Config struct implementation** - Centralized all configuration parameters with validation
- **Structured logging** - Added tracing with user-configurable verbosity levels (error, warn, info, debug, trace) 
- **Enhanced error handling** - Eliminated unwrap() calls, added proper context to all error paths
- **Instrumentation** - Added tracing spans for better debugging and performance monitoring
- **Thread-safe parallel processing** - Fixed Send/Sync issues in frame hashing pipeline

**✅ Previously Well Implemented:**
- Hybrid binary+library architecture 
- Core clustering algorithms with good test coverage
- Multiple output format support with lossless options
- FFmpeg integration with system/sidecar fallback
- Parallel processing for performance
- Basic CLI with proper argument validation

**🔄 Next Priority Items:**
- Fix integration test threshold sensitivity for synthetic video frames
- Add comprehensive edge case testing
- Improve documentation and examples
- Add progress reporting and JSON output modes
