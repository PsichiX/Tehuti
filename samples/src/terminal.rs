use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{DisableMouseCapture, EnableMouseCapture, Event, poll, read},
    execute, queue,
    style::Print,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use std::{fmt::Display, io::Stdout, sync::OnceLock, time::Duration};
use tehuti_diagnostics::log_buffer::LogBuffer;
use vek::Vec2;

static LOG_BUFFER: OnceLock<LogBuffer> = OnceLock::new();

pub struct Terminal {
    output: Stdout,
}

impl Drop for Terminal {
    fn drop(&mut self) {
        execute!(self.output, Show).unwrap();
        execute!(self.output, DisableMouseCapture).unwrap();
        execute!(self.output, LeaveAlternateScreen).unwrap();
        disable_raw_mode().unwrap();
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new(std::io::stdout())
    }
}

impl Terminal {
    pub fn set_global_log_buffer(buffer: LogBuffer) {
        LOG_BUFFER
            .set(buffer)
            .ok()
            .expect("Global log buffer can only be set once");
    }

    pub fn global_log_buffer() -> Option<&'static LogBuffer> {
        LOG_BUFFER.get()
    }

    pub fn draw_global_log_buffer_region(
        &mut self,
        location: impl Into<Vec2<u16>>,
        size: impl Into<Vec2<u16>>,
    ) {
        if let Some(log_buffer) = Self::global_log_buffer() {
            let location = location.into();
            let size = size.into();

            let logs = log_buffer.get_region(size.x as usize, size.y as usize);
            for (i, line) in logs.lines().enumerate() {
                self.display(location + Vec2::new(0, i as u16), line);
            }
        }
    }

    pub fn draw_text_region<D: Display>(
        &mut self,
        location: impl Into<Vec2<u16>>,
        size: impl Into<Vec2<u16>>,
        content: D,
    ) {
        let location = location.into();
        let size = size.into();

        let content = format!("{}", content);
        for (i, line) in content.lines().take(size.y as usize).enumerate() {
            self.display(location + Vec2::new(0, i as u16), line);
        }
    }

    pub fn new(mut output: Stdout) -> Self {
        enable_raw_mode().unwrap();
        execute!(output, EnterAlternateScreen).unwrap();
        execute!(output, EnableMouseCapture).unwrap();
        execute!(output, Hide).unwrap();
        Self { output }
    }

    pub fn events() -> impl Iterator<Item = Event> {
        std::iter::from_fn(|| {
            if poll(Duration::ZERO).unwrap() {
                read().ok()
            } else {
                None
            }
        })
    }

    pub fn begin_draw(&mut self, clear_screen: bool) {
        if clear_screen {
            self.clear_screen();
        }
    }

    pub fn end_draw(&mut self) {
        execute!(self.output).unwrap();
    }

    pub fn clear_screen(&mut self) {
        queue!(self.output, Clear(ClearType::All), MoveTo(0, 0)).unwrap();
    }

    pub fn output(&mut self) -> &mut Stdout {
        &mut self.output
    }

    pub fn display<D: Display>(&mut self, location: impl Into<Vec2<u16>>, content: D) {
        let location = location.into();
        queue!(self.output, MoveTo(location.x, location.y), Print(content)).unwrap();
    }
}
