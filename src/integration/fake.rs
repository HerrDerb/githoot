//! A test double that imports nothing from `integration::herdr`.
//!
//! It compiles only if the trait can be implemented without a single Herdr type, which is the whole
//! promise of the seam, the same promise `portal::fake` keeps for portals.

use super::{Batch, Context, Info, Integration, Kind, Said, Setting};
use crate::portal::PortalKind;

pub struct Echo;

/// How often `installed` was called, so the generic Install can be seen to call it.
pub static INSTALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub static ECHO: Info = Info {
    id: "echo",
    name: "Echo",
    summary: "Says which pull requests it was handed.",
    portals: &[PortalKind::GitHub],
    settings: &[Setting {
        key: "prefix",
        label: "Prefix",
        kind: Kind::Text { placeholder: "echo" },
        help: "What each line starts with.",
        group: "Output",
        live: true,
    }],
    unsupported: None,
};

impl Integration for Echo {
    fn info(&self) -> &'static Info {
        &ECHO
    }

    fn installed(&self, _ctx: &Context, _on: bool) {
        INSTALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn pass(&self, ctx: &Context, batches: &[Batch], dry_run: bool) -> Said {
        let prefix = if ctx.setting("prefix").is_empty() { "echo" } else { ctx.setting("prefix") };
        let mut said = Said::default();
        for batch in batches {
            for entry in &batch.entries {
                said.said.push(format!("{prefix} [{}] {}{}", batch.axis.slug(), entry.url, if dry_run { " (dry)" } else { "" }));
            }
        }
        said
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::integration::batches;
    use crate::portal::fake::FakePortal;
    use crate::portal::types::PrEntry;
    use crate::scheduler::AxisSnapshot;
    use crate::state::PrAxis;

    #[test]
    fn an_integration_can_be_written_without_herdr_and_run_on_the_bars() {
        let snaps = vec![(
            PrAxis::ReviewRequested,
            AxisSnapshot {
                groups: vec![(FakePortal::named("GitHub").info, Some(vec![PrEntry::stub("https://example.invalid/1")]))],
                polled_at: None,
                version: 1,
            },
        )];
        let cfg = Config::from_text("");
        let ctx = Context::new(std::path::Path::new("/nonexistent"), &cfg, "echo");
        let out = Echo.pass(&ctx, &batches(Echo.info(), &snaps, &|_| false), true);
        assert_eq!(out.said, ["echo [requested-reviews] https://example.invalid/1 (dry)"]);
        assert!(ctx.dir.ends_with("integrations/echo"));
    }
}
