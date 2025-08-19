use assert_cmd::prelude::*;
use assert_fs::prelude::*;
use predicates::prelude::*;
use std::process::Command;
use std::path::PathBuf;

/// Helper: synthesizes N PNGs of distinct solid colors.
fn make_synthetic_pngs(dir: &assert_fs::TempDir, n: usize) -> Vec<PathBuf> {
    use image::{ImageBuffer, Rgba};
    let mut out = Vec::new();
    for i in 0..n {
        let w = 64;
        let h = 64;
        let mut img = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(w, h);
        let r = (i as u8).wrapping_mul(37);
        let g = (i as u8).wrapping_mul(73);
        let b = (i as u8).wrapping_mul(17);
        for p in img.pixels_mut() {
            *p = Rgba([r, g, b, 255]);
        }
        let path = dir.child(format!("slide_{i:02}.png"));
        img.save(path.path()).unwrap();
        out.push(path.to_path_buf());
    }
    out
}

fn have_system_ffmpeg() -> bool {
    which::which("ffmpeg").is_ok()
}

#[test]
fn splits_exact_slides_without_transitions() {
    // Arrange: 3-frame slideshow @1s each, hard cuts
    let td = assert_fs::TempDir::new().unwrap();
    let frames = make_synthetic_pngs(&td, 3);
    let input = td.child("in.mp4");

    // Build a slideshow video using ffmpeg; if system ffmpeg isn't present,
    // we can still rely on the binary under test to fetch a sidecar later,
    // but for *building* this tiny input we need some ffmpeg. If missing,
    // skip this test gracefully.
    if !have_system_ffmpeg() {
        eprintln!("Skipping: system ffmpeg missing (only needed to *create* test video).");
        return;
    }

    // Create video at 1 fps from PNGs (hard cuts, no transitions)
    let mut list = String::new();
    for f in &frames {
        list.push_str(&format!("file '{}'\nduration 1.0\n", f.display()));
    }
    // Repeat last frame recommendation to ensure duration is respected
    list.push_str(&format!("file '{}'\n", frames.last().unwrap().display()));
    let concat = td.child("list.txt");
    concat.write_str(&list).unwrap();

    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner","-loglevel","error",
            "-f","concat","-safe","0",
            "-i", concat.path().to_str().unwrap(),
            "-pix_fmt","yuv420p",
            input.path().to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg must create input mp4");

    // Act: run our binary (will use system ffmpeg or auto sidecar)
    let out_dir = td.child("out");
    let mut cmd = Command::cargo_bin("slidesplit").unwrap();
    cmd.arg(input.path())
        .arg("--fps").arg("2.0")
        .arg("--threshold").arg("10")
        .arg("--min-stable-seconds").arg("0.5")
        .arg("--format").arg("png")
        .arg("-o").arg(out_dir.path());

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Wrote 3 slide"));

    // Assert exactly 3 outputs
    out_dir.assert(predicates::path::exists());
    let entries = std::fs::read_dir(out_dir.path()).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("png"))
        .count();
    assert_eq!(entries, 3, "expected exactly 3 output PNGs");
}

/// Ignored by default (uses cross-fade + takes a bit longer).
#[test]
#[ignore]
fn splits_through_crossfade() {
    let td = assert_fs::TempDir::new().unwrap();
    let frames = make_synthetic_pngs(&td, 3);
    let input = td.child("in_fade.mp4");

    if !have_system_ffmpeg() {
        eprintln!("Skipping: system ffmpeg missing (only needed to *create* test video).");
        return;
    }

    // Build a 3-slide video with cross-fades between slides
    // We'll just do: slide0 1s -> fade 0.5s -> slide1 1s -> fade 0.5s -> slide2 1s
    // Implement via a filter_complex script:
    let filter = format!(
        "\
        [0:v]format=rgba,trim=0:1,setpts=PTS-STARTPTS[a0]; \
        [1:v]format=rgba,trim=0:1,setpts=PTS-STARTPTS[a1]; \
        [2:v]format=rgba,trim=0:1,setpts=PTS-STARTPTS[a2]; \
        [a0][a1]xfade=transition=fade:duration=0.5:offset=0.5[b0]; \
        [b0][a2]xfade=transition=fade:duration=0.5:offset=1.5[outv]"
    );

    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner","-loglevel","error",
            "-loop","1","-t","1","-i", frames[0].to_str().unwrap(),
            "-loop","1","-t","1","-i", frames[1].to_str().unwrap(),
            "-loop","1","-t","1","-i", frames[2].to_str().unwrap(),
            "-filter_complex", &filter,
            "-map","[outv]",
            "-pix_fmt","yuv420p",
            input.path().to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg must create input with fades");

    let out_dir = td.child("out");
    let mut cmd = Command::cargo_bin("slidesplit").unwrap();
    cmd.arg(input.path())
        .arg("--fps").arg("4.0")       // denser to sample the fade
        .arg("--threshold").arg("10")  // default
        .arg("--min-stable-seconds").arg("0.8")
        .arg("--format").arg("png")
        .arg("-o").arg(out_dir.path());

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Wrote 3 slide"));

    let entries = std::fs::read_dir(out_dir.path()).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("png"))
        .count();
    assert_eq!(entries, 3, "expected exactly 3 outputs despite the cross-fades");
}
