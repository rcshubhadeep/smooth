#![allow(deprecated)]

use anyhow::{bail, Context, Result};
use polyvoice::{
    models::ModelRegistry,
    pipeline::Pipeline,
    types::{ClusterConfig, DiarizationConfig, Profile},
    vad::VadConfig,
    wav::read_wav,
    FbankOnnxExtractor, SileroVad,
};
use serde::Serialize;
use std::{env, path::PathBuf};

#[derive(Debug, Serialize)]
struct DiarizeOutput {
    turns: Vec<DiarizeTurn>,
    engine: &'static str,
}

#[derive(Debug, Serialize)]
struct DiarizeTurn {
    speaker_id: String,
    start_ms: i64,
    end_ms: i64,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args
        .first()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        print_help();
        return Ok(());
    }
    if args.first().map(String::as_str) != Some("diarize") {
        print_help();
        bail!("missing command: expected `diarize`");
    }

    let input = required_arg(&args, "--input")?;
    let models_cache = required_arg(&args, "--models-cache")?;
    let profile = optional_arg(&args, "--profile")
        .unwrap_or_else(|| "balanced".to_string())
        .parse::<Profile>()
        .context("invalid profile")?;

    if !input.is_file() {
        bail!("input WAV does not exist: {}", input.display());
    }

    let result = run_legacy_pipeline(&input, profile, &models_cache)?;

    let output = DiarizeOutput {
        turns: result
            .turns
            .into_iter()
            .map(|turn| DiarizeTurn {
                speaker_id: normalize_speaker_id(&turn.speaker.to_string()),
                start_ms: seconds_to_ms(turn.time.start),
                end_ms: seconds_to_ms(turn.time.end),
            })
            .collect(),
        engine: "smooth-diarize-polyvoice",
    };

    println!(
        "{}",
        serde_json::to_string(&output).context("failed to serialize diarization output")?
    );
    Ok(())
}

fn run_legacy_pipeline(
    input: &PathBuf,
    profile: Profile,
    models_cache: &PathBuf,
) -> Result<polyvoice::types::DiarizationResult> {
    let registry = ModelRegistry::with_cache_dir(models_cache)
        .with_context(|| format!("failed to open models cache {}", models_cache.display()))?;
    let models = registry
        .ensure_for_profile(profile)
        .context("failed to load Polyvoice profile models")?;
    let extractor = FbankOnnxExtractor::new(&models.embedder_path, profile.embedding_dim(), 1)
        .context("failed to load Polyvoice embedder")?;
    let vad_path = registry
        .ensure("silero_vad")
        .context("failed to load Silero VAD model")?;
    let mut vad = SileroVad::new(&vad_path, 512).context("failed to load Silero VAD")?;
    let pipeline = Pipeline::new(
        DiarizationConfig {
            cluster: ClusterConfig {
                threshold: profile.default_threshold(),
                ..ClusterConfig::default()
            },
            ..DiarizationConfig::default()
        },
        VadConfig::default(),
    );
    let (samples, sample_rate) =
        read_wav(input).with_context(|| format!("failed to read {}", input.display()))?;
    if sample_rate != 16_000 {
        bail!("expected 16 kHz WAV, got {sample_rate} Hz");
    }
    pipeline
        .run(&samples, &extractor, &mut vad)
        .context("Polyvoice diarization failed")
}

fn required_arg(args: &[String], name: &str) -> Result<PathBuf> {
    optional_arg(args, name)
        .map(PathBuf::from)
        .with_context(|| format!("missing required argument {name}"))
}

fn optional_arg(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn normalize_speaker_id(value: &str) -> String {
    let clean = value
        .trim()
        .trim_matches('"')
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");

    if clean.is_empty() {
        "speaker".to_string()
    } else {
        clean
    }
}

fn seconds_to_ms(value: f64) -> i64 {
    (value * 1000.0).round() as i64
}

fn print_help() {
    eprintln!(
        "Usage: smooth-diarize diarize --input <wav> --models-cache <dir> [--profile balanced|mobile]"
    );
}
