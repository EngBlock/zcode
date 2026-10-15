//! View ▸ Performance: startup timings and live memory, including the
//! language server's footprint.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use crate::actions::PickerDismiss;
use crate::perf::{Perf, format_bytes};

pub struct PerfPanel {
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DismissEvent> for PerfPanel {}

impl Focusable for PerfPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PerfPanel {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            _subscription: cx.observe_global::<Perf>(|_, cx| cx.notify()),
        }
    }
}

fn ms(d: Option<std::time::Duration>) -> String {
    d.map(|d| format!("{:.0} ms", d.as_secs_f64() * 1000.))
        .unwrap_or_else(|| "—".into())
}

fn metric(label: &'static str, value: String, cx: &App) -> impl IntoElement {
    h_flex()
        .justify_between()
        .text_sm()
        .child(div().text_color(cx.theme().muted_foreground).child(label))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(value))
}

impl Render for PerfPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let perf = Perf::global(cx);
        let mem = perf.current();
        let history: Vec<u64> = perf.history.iter().map(|s| s.footprint).collect();
        let max = history.iter().copied().max().unwrap_or(1).max(1);
        let report = perf.report_json();
        let startup = perf.startup.clone();
        let peak = perf.peak_footprint.max(mem.lifetime_peak);
        let ls = perf.language_server;

        v_flex()
            .key_context("PerfPanel")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &PickerDismiss, _, cx| cx.emit(DismissEvent)))
            .w(px(520.))
            .p_4()
            .gap_3()
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Performance"))
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child("STARTUP"))
            .child(metric("Process launch → main()", ms(startup.exec_to_main), cx))
            .child(metric("main() → window open", ms(startup.main_to_window), cx))
            .child(metric("main() → first frame", ms(startup.main_to_first_frame), cx))
            .child(metric("Total to first frame", ms(startup.total()), cx))
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child("MEMORY (PHYSICAL FOOTPRINT)"))
            .child(metric("zcode (now)", format_bytes(mem.footprint), cx))
            .child(metric("zcode (idle baseline, 30 s)", format_bytes(perf.baseline_footprint()), cx))
            .child(metric("zcode peak", format_bytes(peak), cx))
            .child(metric("zcode resident", format_bytes(mem.resident), cx))
            .child(metric(
                "sourcekit-lsp + SourceKitService",
                ls.map(|(pid, s)| format!("{} (pid {pid})", format_bytes(s.footprint)))
                    .unwrap_or_else(|| "not running".into()),
                cx,
            ))
            .child(
                h_flex()
                    .h(px(56.))
                    .items_end()
                    .gap(px(1.))
                    .p_1()
                    .rounded(theme.radius)
                    .bg(theme.secondary)
                    .children(history.iter().map(|&value| {
                        let h = (value as f32 / max as f32 * 48.).max(1.);
                        div().w(px(3.)).h(px(h)).bg(theme.primary.opacity(0.7))
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Sampled every 2 s. While the window is frontmost, macOS attributes ~200 MB of transient compositor memory to each redraw (a bare GPUI window shows the same); the idle baseline excludes it. Run `zcode --startup-bench` for a scripted measurement."),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("copy-perf")
                            .label("Copy as JSON")
                            .small()
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(report.clone()));
                            }),
                    )
                    .child(
                        Button::new("close-perf")
                            .label("Done")
                            .small()
                            .primary()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    ),
            )
    }
}
