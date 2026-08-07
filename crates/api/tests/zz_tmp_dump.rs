use api::render::{JournalNote, PullView};
use pull::session::Day;

#[test]
fn dump() {
    let today = Day::new(2026, 8, 7).unwrap();
    let view = PullView {
        today,
        targets: &[],
        capture: None,
        no_capture: "none",
        journal: JournalNote::default(),
        halt: None,
        notes: &[],
    };
    let html = api::render::pull_page(&view);
    std::fs::write("/private/tmp/claude-501/-Users-parthi-IdeaProjects-brutex-rs/d0f5978c-1da1-4efb-82c5-1a7b9389076a/scratchpad/pull.html", &html).unwrap();
}
