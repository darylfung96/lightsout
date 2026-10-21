//! `video-prune` — the safe render-intermediate retention job.
//!
//! These are additive to the 261 contract checks. The fixture is a fake video
//! checkout: only render directories for posted episodes with a master may be
//! removed, and the last N episodes stay intact.

mod common;

use common::*;
use serde_json::json;

fn setup_video(s: &Sandbox) -> String {
    for ep in ["016", "017", "018", "019", "020"] {
        s.write(
            &format!("video/episodes/{ep}/comp/my-video/renders/take.mp4"),
            "render bytes",
        );
        s.write(
            &format!("video/episodes/{ep}/comp/my-video/assets/asset.bin"),
            "asset bytes",
        );
        s.write(
            &format!("video/episodes/{ep}/media/shot01.mp4"),
            "media bytes",
        );
        s.write(
            &format!("video/episodes/{ep}/audio/narration.wav"),
            "audio bytes",
        );
        s.write(&format!("video/outbox/{ep}-master.mp4"), "master bytes");
    }
    s.write(
        "video/pipeline/publish/queue.json",
        &json!({
            "016": {"status": "posted"},
            "017": {"status": "built"},
            "018": {"status": "posted"},
            "019": {"status": "posted"},
            "020": {"status": "posted"}
        })
        .to_string(),
    );
    s.path("video").to_string_lossy().into_owned()
}

#[test]
fn a_plan_is_read_only_even_when_apply_is_requested() {
    let s = Sandbox::new("video-prune-plan");
    let video = setup_video(&s);
    let before_state = s.count_files("jobs/video-prune/state");
    let before_out = s.count_files("jobs/video-prune/out");
    let out = s.run_env(
        &["plan", "video-prune"],
        &[
            ("VIDEO_REPO", video.as_str()),
            ("PRUNE_APPLY", "1"),
            ("PRUNE_KEEP", "3"),
            ("PRUNE_MIN_AGE_HOURS", "0"),
        ],
    );
    assert_eq!(code(&out), 0, "plan: {}", combined(&out));
    assert!(
        s.exists("video/episodes/016/comp/my-video/renders/take.mp4"),
        "plan deleted nothing"
    );
    assert_eq!(
        s.count_files("jobs/video-prune/state"),
        before_state,
        "plan wrote no state file"
    );
    assert_eq!(
        s.count_files("jobs/video-prune/out"),
        before_out,
        "plan wrote no output file"
    );
    assert!(
        !s.exists(&format!("jobs/video-prune/state/prune-{}.tsv", s.today())),
        "plan wrote no manifest"
    );
}

#[test]
fn a_dry_run_writes_a_plan_and_deletes_nothing() {
    let s = Sandbox::new("video-prune-dry");
    let video = setup_video(&s);
    let out = s.run_env(
        &["run", "video-prune"],
        &[
            ("VIDEO_REPO", &video),
            ("PRUNE_APPLY", "0"),
            ("PRUNE_KEEP", "3"),
            ("PRUNE_MIN_AGE_HOURS", "0"),
        ],
    );
    assert_eq!(code(&out), 0, "dry-run: {}", combined(&out));
    let text = combined(&out);
    assert!(text.contains("would remove"), "plan is printed: {text}");
    assert!(text.contains("016"), "old episode is named: {text}");
    assert!(
        !text.contains("would remove\t017"),
        "unposted episode is skipped: {text}"
    );

    let plan = format!("jobs/video-prune/state/prune-{}.tsv", s.today());
    assert!(s.is_file(&plan), "plan manifest exists: {plan}");
    assert!(
        s.exists("video/episodes/016/comp/my-video/renders/take.mp4"),
        "dry-run kept the render"
    );
    assert!(
        s.exists("video/episodes/016/comp/my-video/assets/asset.bin"),
        "dry-run kept the assets"
    );
    assert!(
        s.exists("video/outbox/016-master.mp4"),
        "dry-run kept the master"
    );
}

#[test]
fn apply_removes_only_old_posted_renders() {
    let s = Sandbox::new("video-prune-apply");
    let video = setup_video(&s);
    let out = s.run_env(
        &["run", "video-prune"],
        &[
            ("VIDEO_REPO", &video),
            ("PRUNE_APPLY", "1"),
            ("PRUNE_KEEP", "3"),
            ("PRUNE_MIN_AGE_HOURS", "0"),
        ],
    );
    assert_eq!(code(&out), 0, "apply: {}", combined(&out));

    assert!(
        !s.exists("video/episodes/016/comp/my-video/renders"),
        "old posted render removed"
    );
    assert!(
        s.exists("video/episodes/016/comp/my-video/assets/asset.bin"),
        "assets stay"
    );
    assert!(
        s.exists("video/episodes/016/media/shot01.mp4"),
        "media stays"
    );
    assert!(
        s.exists("video/episodes/016/audio/narration.wav"),
        "audio stays"
    );
    assert!(s.exists("video/outbox/016-master.mp4"), "master stays");

    assert!(
        s.exists("video/episodes/017/comp/my-video/renders/take.mp4"),
        "an unposted episode is kept"
    );
    assert!(
        s.exists("video/episodes/018/comp/my-video/renders/take.mp4"),
        "the last N episodes are kept"
    );
    assert!(
        s.is_file(&format!("jobs/video-prune/state/applied-{}.tsv", s.today())),
        "applied manifest exists"
    );
    assert!(
        s.exists(&format!("jobs/video-prune/state/done/{}", s.today())),
        "a real apply marks the day done"
    );
}

#[test]
#[cfg(unix)]
fn a_symlinked_render_directory_is_refused() {
    let s = Sandbox::new("video-prune-symlink");
    let video = setup_video(&s);
    s.write("outside/renders/take.mp4", "outside bytes");
    std::os::unix::fs::symlink(s.path("outside"), s.path("video/episodes/016/comp/link")).unwrap();

    let out = s.run_env(
        &["run", "video-prune"],
        &[
            ("VIDEO_REPO", video.as_str()),
            ("PRUNE_APPLY", "1"),
            ("PRUNE_KEEP", "3"),
            ("PRUNE_MIN_AGE_HOURS", "0"),
        ],
    );
    assert_eq!(code(&out), 0, "symlink apply: {}", combined(&out));
    assert!(
        s.exists("outside/renders/take.mp4"),
        "the symlink target is untouched"
    );
    assert!(
        !s.exists("video/episodes/016/comp/my-video/renders"),
        "the ordinary render directory is still pruned"
    );
}
