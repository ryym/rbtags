use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

static LOG_FILE: Mutex<Option<String>> = Mutex::new(None);

pub fn init(path: &str) {
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
    {
        let _ = writeln!(f, "--- rbtags log started ---");
    }
    *LOG_FILE.lock().unwrap() = Some(path.to_string());
}

pub fn write(msg: std::fmt::Arguments<'_>) {
    let guard = LOG_FILE.lock().unwrap();
    let Some(path) = guard.as_ref() else { return };
    if let Ok(mut f) = OpenOptions::new().append(true).open(path) {
        let _ = writeln!(f, "{msg}");
    }
}
