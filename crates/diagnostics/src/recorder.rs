use chrono::Utc;
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};
use tracing::Subscriber;
use tracing_subscriber::{
    Layer,
    filter::Targets,
    fmt::{MakeWriter, layer},
    registry::LookupSpan,
};

#[derive(Clone)]
pub struct Recorder {
    directory: PathBuf,
    file_name: String,
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new("./")
    }
}

impl Recorder {
    pub fn new(directory: impl AsRef<Path>) -> Self {
        Self {
            directory: directory.as_ref().to_path_buf(),
            file_name: "recording".to_owned(),
        }
    }

    pub fn file_name(mut self, file_name: impl ToString) -> Self {
        self.file_name = file_name.to_string();
        self
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
        layer()
            .json()
            .with_writer(self)
            .with_target(true)
            .with_filter(filter)
    }
}

impl<'a> MakeWriter<'a> for Recorder {
    type Writer = File;

    fn make_writer(&'a self) -> Self::Writer {
        let mut file_path = self.directory.to_owned();
        file_path.push(format!(
            "{}.{}.log",
            self.file_name,
            Utc::now().format("%Y%m%d%H%M%S")
        ));
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .open(&file_path)
            .unwrap_or_else(|err| {
                panic!(
                    "Could not open diagnostics log: {:?}. Error: {:?}",
                    file_path, err
                )
            })
    }
}
