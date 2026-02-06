use crossterm::event::{Event, KeyCode, KeyEventKind};
use textwrap::Options;
use vek::Vec2;

pub fn is_key(events: &[Event], key_code: KeyCode, kind: Option<KeyEventKind>) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::Key(key_event) if key_event.code == key_code && kind.is_none_or(|k| key_event.kind == k)))
}

pub fn text_wrap(text: &str, width: usize) -> String {
    textwrap::wrap(text, Options::new(width)).join("\n")
}

pub fn text_size(text: &str) -> Vec2<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let height = lines.len();
    let width = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    Vec2::new(width, height)
}
