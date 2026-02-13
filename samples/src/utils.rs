use crossterm::event::{Event, KeyCode, KeyEventKind};
use std::collections::HashMap;
use textwrap::Options;
use vek::Vec2;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KeyState {
    pub is_down: bool,
    pub was_down: bool,
}

impl KeyState {
    pub fn is_down(&self) -> bool {
        self.is_down
    }

    pub fn is_up(&self) -> bool {
        !self.is_down
    }

    pub fn just_pressed(&self) -> bool {
        self.is_down && !self.was_down
    }

    pub fn just_released(&self) -> bool {
        !self.is_down && self.was_down
    }
}

#[derive(Debug, Default)]
pub struct Keys {
    cache: HashMap<KeyCode, KeyState>,
}

impl Keys {
    pub fn update(&mut self, events: &[Event]) {
        for state in self.cache.values_mut() {
            state.was_down = state.is_down;
        }
        for event in events {
            if let Event::Key(key_event) = event {
                let is_down = match key_event.kind {
                    KeyEventKind::Press | KeyEventKind::Repeat => true,
                    KeyEventKind::Release => false,
                };
                self.cache
                    .entry(key_event.code)
                    .and_modify(|state| state.is_down = is_down)
                    .or_insert(KeyState {
                        is_down,
                        was_down: false,
                    });
            }
        }
    }

    pub fn get(&self, key_code: KeyCode) -> KeyState {
        self.cache.get(&key_code).cloned().unwrap_or_default()
    }
}

pub fn text_wrap(text: &str, width: usize, height: usize) -> String {
    let mut result = textwrap::wrap(text, Options::new(width));
    result.truncate(height);
    result.join("\n")
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
