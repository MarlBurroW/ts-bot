/// Build a Rustpotter wakeword model (.rpw) from WAV sample files.
///
/// Usage: cargo run --example build_wakeword
use rustpotter::{WakewordRefBuildFromFiles, WakewordRef, WakewordSave};

fn main() {
    let samples: Vec<String> = vec![
        "models/samples/marlbot_alloy_16k.wav".to_string(),
        "models/samples/marlbot_echo_16k.wav".to_string(),
        "models/samples/marlbot_fable_16k.wav".to_string(),
        "models/samples/marlbot_nova_16k.wav".to_string(),
        "models/samples/marlbot_onyx_16k.wav".to_string(),
        "models/samples/marlbot_shimmer_16k.wav".to_string(),
    ];

    println!("Building wakeword ref from {} samples...", samples.len());
    let wakeword = WakewordRef::new_from_sample_files(
        "marlbot".to_string(),
        Some(0.5),       // threshold per sample
        Some(0.5),       // avg_threshold
        samples,
        16,              // mfcc_size
    ).expect("Failed to build wakeword ref");

    let output = "models/marlbot.rpw";
    wakeword.save_to_file(output).expect("Failed to save wakeword model");
    println!("Saved wakeword model to {}", output);
}
