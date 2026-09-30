use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use radiust_core::runtime::{OperationEvent, OperationStage};
use std::collections::BTreeMap;
use std::time::Duration;

/// One bar per operation: nested discovery, acquisition, and decode events
/// cannot overwrite each other's totals. Only safe event fields are displayed.
pub(crate) struct Progress {
    display: MultiProgress,
    bars: BTreeMap<u64, ProgressBar>,
    unicode: bool,
}

impl Progress {
    pub(crate) fn new() -> Self {
        Self {
            display: MultiProgress::with_draw_target(ProgressDrawTarget::stderr()),
            bars: BTreeMap::new(),
            unicode: !std::env::var("TERM").is_ok_and(|term| term == "dumb"),
        }
    }

    pub(crate) fn event(&mut self, event: &OperationEvent) {
        let unicode = self.unicode;
        let bar = self.bars.entry(event.operation_id.get()).or_insert_with(|| {
            let bar = self.display.add(
                ProgressBar::new_spinner()
                    .with_style(style(false, unicode))
                    .with_prefix(format!("radiust: {}", event.operation.as_str()))
                    .with_message("started"),
            );
            bar.enable_steady_tick(Duration::from_millis(100));
            bar
        });
        match event.stage {
            OperationStage::Started => {}
            OperationStage::Progress => {
                if let Some(progress) = event.progress {
                    if bar.length() != progress.total {
                        bar.set_style(style(progress.total.is_some(), unicode));
                        if let Some(total) = progress.total {
                            bar.set_length(total);
                            // Counts change on events; elapsed time only needs
                            // one refresh per second once the total is known.
                            bar.enable_steady_tick(Duration::from_secs(1));
                        } else {
                            bar.unset_length();
                            bar.enable_steady_tick(Duration::from_millis(100));
                        }
                    }
                    if bar.position() != progress.completed {
                        bar.set_position(progress.completed);
                    }
                    if bar.message() != "working" {
                        bar.set_message("working");
                    }
                }
            }
            OperationStage::Completed | OperationStage::Cancelled | OperationStage::Failed => {
                bar.set_style(
                    ProgressStyle::with_template("{prefix} {msg} [{elapsed_precise}]")
                        .expect("valid progress template"),
                );
                bar.finish_with_message(match event.stage {
                    OperationStage::Completed => "complete",
                    OperationStage::Cancelled => "cancelled",
                    _ => "failed",
                });
            }
        }
    }

    pub(crate) fn lagged(&self) {
        let _ = self.display.println("radiust: progress events skipped");
    }
}

fn style(total: bool, unicode: bool) -> ProgressStyle {
    let style = ProgressStyle::with_template(if total {
        "{prefix} {wide_bar} {pos}/{len} [{elapsed_precise}]"
    } else {
        "{prefix} {spinner} {msg} [{elapsed_precise}]"
    })
    .expect("valid progress template");
    if unicode {
        style
            .progress_chars("━━─")
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
    } else {
        style.progress_chars("=>-").tick_chars("|/-\\")
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        for bar in self.bars.values() {
            if !bar.is_finished() {
                bar.finish_and_clear();
            }
        }
        // Clear retained finished rows before the final report is written.
        let _ = self.display.clear();
    }
}
