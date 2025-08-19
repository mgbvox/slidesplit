use slidesplit::{cluster_frames, merge_short_clusters, FrameEntry};
use img_hash::ImageHash;
use std::path::PathBuf;

fn h64(u: u64) -> ImageHash {
    // Construct a synthetic 64-bit hash from an integer (big-endian order)
    let bytes = u.to_be_bytes();
    ImageHash::from_bytes(&bytes).unwrap()
}


#[test]
fn clusters_split_when_distance_exceeds_threshold() {
    // Build hashes so that frames 0..=4 are similar,
    // 5..=9 similar to each other but far from 0..=4.
    let mut frames = Vec::new();
    for i in 0..5 {
        frames.push(FrameEntry {
            idx: i,
            path: PathBuf::from(format!("f{i}.png")),
            hash: h64(0xAAAA_AAAA_AAAA_AAAA ^ i as u64),
        });
    }
    for i in 5..10 {
        frames.push(FrameEntry {
            idx: i,
            path: PathBuf::from(format!("f{i}.png")),
            hash: h64(0x5555_5555_5555_5555 ^ i as u64),
        });
    }

    // A modest threshold splits into two clusters
    let clusters = cluster_frames(&frames, 8);
    assert_eq!(clusters.len(), 2);
    assert_eq!(clusters[0].first().copied(), Some(0));
    assert_eq!(clusters[0].last().copied(), Some(4));
    assert_eq!(clusters[1].first().copied(), Some(5));
    assert_eq!(clusters[1].last().copied(), Some(9));
}

#[test]
fn merge_short_clusters_stabilizes_crossfade_blips() {
    // Simulate: 8 frames slide A, then 3 frames transition, then 8 frames slide B.
    // Transition hashes sit “between” the two anchors.
    let mut frames = Vec::new();
    // A cluster
    for i in 0..8 {
        frames.push(FrameEntry {
            idx: i,
            path: PathBuf::from(format!("a{i}.png")),
            hash: h64(0x0000_0000_0000_0000 ^ i as u64),
        });
    }
    // Transition mini-cluster
    for i in 8..11 {
        frames.push(FrameEntry {
            idx: i,
            path: PathBuf::from(format!("t{i}.png")),
            hash: h64(0x0F0F_0F0F_0F0F_0F0F ^ i as u64),
        });
    }
    // B cluster
    for i in 11..19 {
        frames.push(FrameEntry {
            idx: i,
            path: PathBuf::from(format!("b{i}.png")),
            hash: h64(0xFFFF_FFFF_FFFF_FFFF ^ i as u64),
        });
    }

    // First pass: we likely get three clusters
    let mut clusters = cluster_frames(&frames, 8);
    assert_eq!(clusters.len(), 3);

    // Merge with min_stable_seconds so the 3-frame transition collapses
    // Suppose fps=2.0 => min_len = ceil(1.0*2.0)=2 frames; use 1.5s to be stricter
    merge_short_clusters(&mut clusters, &frames, 1.5, 2.0, 8);
    assert_eq!(clusters.len(), 2, "Transition cluster should be merged away");
}
