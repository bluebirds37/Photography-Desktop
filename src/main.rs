use gpui::prelude::*;
use gpui::*;

use timelapse_assistant::ui::{Assets, TimelapseApp};

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1480.0), px(920.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("延时摄影助手".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            window_min_size: Some(size(px(1180.0), px(720.0))),
            ..Default::default()
        };
        cx.open_window(options, |_, cx| cx.new(|cx| TimelapseApp::new(cx)))
            .expect("failed to open window");
        cx.activate(true);
    });
}
