pub trait TranscriptProcessor {
    fn process(&self, transcript: &str) -> String;
}

/// Keeps the MVP deterministic while VolcEngine supplies punctuation and ITN.
/// A semantic/LLM processor can replace this implementation without changing
/// the capture, overlay, shortcut, or cursor-insertion lifecycle.
pub struct AsrCleanup;

impl TranscriptProcessor for AsrCleanup {
    fn process(&self, transcript: &str) -> String {
        transcript.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

pub fn process_final(transcript: &str) -> String {
    AsrCleanup.process(transcript)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_collapses_transport_whitespace() {
        assert_eq!(process_final("  hello   world\n"), "hello world");
    }
}
