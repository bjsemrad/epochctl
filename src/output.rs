use serde_json::Value;

/// How results are printed. `--json` makes every command emit one JSON object so scripts have a
/// single shape to parse; the default is terse human text suited to a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
}

impl Format {
    pub fn new(json: bool) -> Self {
        if json {
            Self::Json
        } else {
            Self::Human
        }
    }

    pub fn is_json(self) -> bool {
        self == Self::Json
    }

    /// Print a machine payload in JSON mode, or run `human` for the terminal rendering.
    pub fn emit(self, value: &Value, human: impl FnOnce()) {
        if self.is_json() {
            match serde_json::to_string_pretty(value) {
                Ok(text) => println!("{text}"),
                Err(err) => eprintln!("epochctl: could not render JSON: {err}"),
            }
        } else {
            human();
        }
    }
}

/// Right-pad `value` to `width` display columns.
pub fn pad(value: &str, width: usize) -> String {
    let len = value.chars().count();
    if len >= width {
        value.to_string()
    } else {
        format!("{value}{}", " ".repeat(width - len))
    }
}
