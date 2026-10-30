use std::io::Write;
use std::path::Path;

use austen_lib::observe::{RunEvent, RunObserver};

enum Sink {
    Stdout,
    File(std::fs::File),
}

pub struct JsonlRenderer(std::sync::Mutex<Sink>);

impl JsonlRenderer {
    pub fn open(dest: &Path) -> std::io::Result<Self> {
        let sink = if dest.as_os_str() == "-" {
            Sink::Stdout
        } else {
            Sink::File(std::fs::File::create(dest)?)
        };
        Ok(Self(std::sync::Mutex::new(sink)))
    }
}

impl RunObserver for JsonlRenderer {
    fn event(&self, event: RunEvent) {
        let Ok(mut line) = serde_json::to_string(&event) else {
            return;
        };
        line.push('\n');
        let mut sink = self.0.lock().expect("the event sink lock");
        let _ = match &mut *sink {
            Sink::Stdout => std::io::stdout().write_all(line.as_bytes()),
            Sink::File(f) => f.write_all(line.as_bytes()),
        };
    }
}
