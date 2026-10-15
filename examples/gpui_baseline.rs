//! A bare GPUI Kit window that redraws every 2 s, used as a baseline when
//! measuring zcode's memory. Flags: --activate --theme --bg --assets --icon.
//! On macOS 27, an *active* GPUI window shows ~226 MB of transient GPU memory
//! per redraw that the system reclaims within a second; inactive windows do not.

use gpui_kit::component::{ActiveTheme as _, Icon, Theme, ThemeMode};
use gpui_kit::assets::IconName;
use gpui_kit::*;
struct Hello(u64, bool, bool);
impl Render for Hello {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut el = div().size_full().child(format!("hello {}", self.0));
        if self.1 { el = el.bg(cx.theme().background).text_color(cx.theme().foreground); }
        if self.2 { el = el.child(Icon::new(IconName::Folder)); }
        el
    }
}
fn main() {
    let t0 = std::time::Instant::now();
    let has = |f: &str| std::env::args().any(|a| a == f);
    let (theme, bg, icon) = (has("--theme"), has("--bg"), has("--icon"));
    let app = gpui_kit::application();
    let app = if has("--assets") { app.with_assets(zcode::assets::Assets) } else { app };
    app.run(move |cx| {
        eprintln!("app running {:?}", t0.elapsed());
        gpui_kit::init(cx);
        eprintln!("init {:?}", t0.elapsed());
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1440.), px(900.)), cx))),
            ..Default::default()
        };
        let (_, view) = gpui_kit::open_window(options, cx, |window, cx| {
            if theme { Theme::change(ThemeMode::Dark, Some(window), cx); }
            cx.new(|_| Hello(0, bg, icon))
        }).unwrap();
        eprintln!("window opened {:?}", t0.elapsed());
        if has("--quit") {
            let window = cx.windows()[0];
            window.update(cx, move |_, window, _| window.on_next_frame(move |window, _| window.on_next_frame(move |_, cx| { eprintln!("first frame {:?}", t0.elapsed()); cx.quit(); }))).ok();
        }
        if std::env::args().any(|a| a == "--activate") { cx.activate(true); }
        cx.spawn(async move |cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(2000)).await;
            view.update(cx, |v, cx| { v.0 += 1; cx.notify(); });
        }).detach();
    });
}
