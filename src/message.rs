#[derive(Debug, Clone)]
pub enum Message {
    // (idx, output, time)
    SurfaceCommit(u32, String, u64),

    // Output name, time
    FrameStart(String, u64),

    // time
    FrameRepaint(String, u64),

    // time
    FrameRepaintDone(String, u64),

    //
    Refresh,

    // GUI
    SliderChanged(f64),
    // GUI
    PeriodicRefreshChanged(bool),
}
