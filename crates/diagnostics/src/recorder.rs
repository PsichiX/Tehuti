use chrono::Utc;
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tracing::Subscriber;
use tracing_subscriber::{Layer, filter::Targets, fmt::layer, registry::LookupSpan};

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
        let file_name = std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "recording".to_owned());
        Self {
            directory: directory.as_ref().to_path_buf(),
            file_name,
        }
    }

    pub fn file_name(mut self, file_name: impl ToString) -> Self {
        self.file_name = file_name.to_string();
        self
    }

    pub fn extend_file_name(mut self, extension: impl ToString) -> Self {
        self.file_name = format!("{}-{}", self.file_name, extension.to_string());
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

        let mut file_path = self.directory.to_owned();
        file_path.push(format!(
            "{}.{}.log",
            self.file_name,
            Utc::now().format("%Y-%m-%d_%H-%M-%S")
        ));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .unwrap_or_else(|err| {
                panic!(
                    "Could not open diagnostics log: {:?}. Error: {:?}",
                    file_path, err
                )
            });

        layer()
            .json()
            .with_writer(Mutex::new(file))
            .with_target(true)
            .with_filter(filter)
    }
}
