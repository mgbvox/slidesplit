use img_hash::ImageHash;
use std::path::PathBuf;

#[derive(Clone)]
pub struct FrameEntry {
    pub idx: usize,
    pub path: PathBuf,
    pub hash: ImageHash,
}

/// Initial clustering: anchor strategy
pub fn cluster_frames(frames: &[FrameEntry], threshold: u32) -> Vec<Vec<usize>> {
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
pub fn merge_short_clusters(
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
