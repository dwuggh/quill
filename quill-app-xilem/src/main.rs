use xilem::{EventLoop, winit::error::EventLoopError};

fn main() -> Result<(), EventLoopError> {
    quill_app_xilem::run(EventLoop::builder())
}
