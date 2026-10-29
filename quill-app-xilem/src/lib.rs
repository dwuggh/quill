//! Quill's shared desktop and Android application.

mod app;
mod widget;

pub use app::run;

/// NativeActivity entry point used by Android's activity glue.
#[cfg(target_os = "android")]
// SAFETY: This library owns the single android_main symbol expected by android-activity.
#[unsafe(no_mangle)]
fn android_main(app: xilem::winit::platform::android::activity::AndroidApp) {
    use xilem::EventLoop;
    use xilem::winit::platform::android::EventLoopBuilderExtAndroid;

    let mut event_loop = EventLoop::builder();
    event_loop.with_android_app(app);
    if let Err(error) = run(event_loop) {
        tracing::error!(?error, "Quill's Android event loop stopped");
    }
}
