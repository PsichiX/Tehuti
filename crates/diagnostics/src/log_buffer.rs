use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use textwrap::{Options, wrap};
use tracing::Subscriber;
use tracing_subscriber::{
    Layer,
    filter::Targets,
    fmt::{MakeWriter, layer},
    registry::LookupSpan,
};

#[derive(Clone)]
pub struct LogBuffer {
    inner: Arc<Mutex<VecDeque<String>>>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    pub fn into_layer<S: Subscriber + for<'span> LookupSpan<'span>>(
        self,
        filter: &str,
    ) -> impl Layer<S> {
        let filter = match filter.parse::<Targets>() {
            Ok(filter) => filter,
            Err(err) => {
                println!("Failed to parse tracing filter: {err}");
                Default::default()
            }
        };
        layer().with_writer(self.clone()).with_filter(filter)
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        let mut guard = self.inner.lock().unwrap();
        while guard.len() > capacity {
            guard.pop_front();
        }
    }

    pub fn get_lines(&self) -> Vec<String> {
        self.inner.lock().unwrap().iter().cloned().collect()
    }

    pub fn get_region(&self, width: usize, height: usize) -> String {
        let guard = self.inner.lock().unwrap();
        let lines = guard.iter().take(height).cloned().collect::<Vec<_>>();
        let mut result = VecDeque::new();
        for line in &lines {
            result.extend(wrap(line, Options::new(width)));
        }
        while result.len() > height {
            result.pop_front();
        }
        result.into_iter().collect::<Vec<_>>().join("\n")
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter {
            buffer: self.clone(),
        }
    }
}

pub struct LogWriter {
    buffer: LogBuffer,
}

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let s = String::from_utf8_lossy(buf).to_string();
        let mut guard = self.buffer.inner.lock().unwrap();

        for line in s.lines() {
            if guard.len() == self.buffer.capacity {
                guard.pop_front();
            }
            guard.push_back(line.to_string());
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
