use super::*;

#[test]
fn listening_limits_are_bounded_defaults_and_can_be_overridden() {
    let limits = Config::default().jarvis.listening;
    assert_eq!(limits.max_audio_seconds_per_hour, 600);
    assert_eq!(limits.max_classifications_per_hour, 120);
    assert_eq!(limits.max_agent_turns_per_hour, 20);
    let config: Config = toml::from_str(
        "[jarvis.listening]\nmax_agent_turns_per_hour = 3\naction_probability = 0.99",
    )
    .unwrap();
    assert_eq!(config.jarvis.listening.max_agent_turns_per_hour, 3);
    assert_eq!(config.jarvis.listening.action_probability, 0.99);
}

#[test]
fn jarvis_endpoint_is_optional_and_serializes() {
    assert!(Config::default().jarvis.speech.endpoint.is_none());
    let config: Config = toml::from_str(
        "[jarvis.speech]\nendpoint = 'http://127.0.0.1:8765/v1/audio/speech'\nmodel = 'custom-model'\nvoice = 'custom-voice'\napi_key_env = 'MY_TTS_KEY'",
    ).unwrap();
    assert_eq!(
        config.jarvis.speech.endpoint.as_deref(),
        Some("http://127.0.0.1:8765/v1/audio/speech")
    );
    assert_eq!(config.jarvis.speech.model, "custom-model");
    assert_eq!(config.jarvis.speech.voice, "custom-voice");
    assert_eq!(
        config.jarvis.speech.api_key_env.as_deref(),
        Some("MY_TTS_KEY")
    );
    let roundtrip: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    assert_eq!(
        roundtrip.jarvis.speech.endpoint,
        config.jarvis.speech.endpoint
    );
    assert!(CONFIG_ENV_KEYS.contains(&"JCODE_JARVIS_SPEECH_ENDPOINT"));
}
