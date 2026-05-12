//! Multi-Modal Support for CarpAI
//!
//! Extends the LLM service to handle:
//! - **Vision**: Image understanding and analysis (screenshots, diagrams, UI mockups)
//! - **Audio**: Speech-to-text, audio understanding, voice commands
//! - **Video**: Video frame analysis (future)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────┐     ┌──────────────────┐     ┌─────────────────┐
//! │   Client    │────▶│  MultiModalRouter│────▶│   LLM Provider   │
//! │ (IDE/CLI)   │     │                  │     │  (GPT-4V/Claude) │
//! └─────────────┘     └──────────────────┘     └─────────────────┘
//!                            │
//!              ┌────────────┼────────────┐
//!              ▼            ▼            ▼
//!        ┌──────────┐ ┌──────────┐ ┌──────────┐
//!        │ Vision   │ │  Audio   │ │ Encoder  │
//!        │ Processor│ │ Processor│ │ Manager  │
//!        └──────────┘ └──────────┘ └──────────┘
//! ```

use std::sync::Arc;
use serde::{Deserialize, Serialize};
use anyhow::{Result, Context};
use tracing::{info, debug, instrument};
use tokio::io::AsyncReadExt;
use base64::{Engine as _, engine::general_purpose::STANDARD};

/// Supported modalities
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Modality {
    Text,
    Image,
    Audio,
    Video,
}

impl std::fmt::Display for Modality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text => write!(f, "text"),
            Self::Image => write!(f, "image"),
            Self::Audio => write!(f, "audio"),
            Self::Video => write!(f, "video"),
        }
    }
}

/// Multi-modal content part
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    pub modality: Modality,
    
    /// For text: the text content
    #[serde(default)]
    pub text: Option<String>,
    
    /// For image: base64 encoded image data or URL
    #[serde(default)]
    pub image_data: Option<ImageData>,
    
    /// For audio: base64 encoded audio data or URL
    #[serde(default)]
    pub audio_data: Option<AudioData>,
    
    /// Metadata about this content part
    #[serde(default)]
    pub metadata: ContentMetadata,
}

/// Image data with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    /// Base64 encoded image data
    pub base64: String,
    
    /// Media type (e.g., "image/png", "image/jpeg")
    pub media_type: String,
    
    /// Image dimensions (width x height)
    #[serde(default)]
    pub dimensions: Option<(u32, u32)>,
    
    /// File size in bytes
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

/// Audio data with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioData {
    /// Base64 encoded audio data
    pub base64: String,
    
    /// Media type (e.g., "audio/wav", "audio/mp3")
    pub media_type: String,
    
    /// Duration in seconds
    #[serde(default)]
    pub duration_secs: Option<f64>,
    
    /// Sample rate (Hz)
    #[serde(default)]
    pub sample_rate: Option<u32>,
    
    /// Number of channels
    #[serde(default)]
    pub channels: Option<u8>,
}

/// Content metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContentMetadata {
    /// Original filename (if from file upload)
    #[serde(default)]
    pub filename: Option<String>,
    
    /// MIME type
    #[serde(default)]
    pub mime_type: Option<String>,
    
    /// Language code (for audio/text)
    #[serde(default)]
    pub language: Option<String>,
    
    /// Timestamp when captured/generated
    #[serde(default)]
    pub timestamp: Option<i64>,
    
    /// Custom key-value pairs
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, String>,
}

/// Multi-modal request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiModalRequest {
    /// Session ID for conversation continuity
    pub session_id: String,
    
    /// List of content parts (text + images + audio)
    pub parts: Vec<ContentPart>,
    
    /// Model to use (must support multi-modal)
    pub model: String,
    
    /// Generation parameters
    #[serde(default)]
    pub params: GenerationParams,
    
    /// Request type/context
    #[serde(default)]
    pub context_type: RequestContext,
}

/// Generation parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationParams {
    /// Maximum tokens to generate
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    
    /// Temperature (0.0 - 2.0)
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    
    /// Top-p sampling
    #[serde(default = "default_top_p")]
    pub top_p: f64,
    
    /// Enable streaming response
    #[serde(default = "default_true")]
    pub stream: bool,
}

fn default_max_tokens() -> u32 { 4096 }
fn default_temperature() -> f64 { 0.7 }
fn default_top_p() -> f64 { 1.0 }
fn default_true() -> bool { true }

impl Default for GenerationParams {
    fn default() -> Self {
        Self {
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            top_p: default_top_p(),
            stream: default_true(),
        }
    }
}

/// Request context type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RequestContext {
    /// General chat/conversation
    Chat,
    
    /// Code explanation with screenshot
    CodeExplanation,
    
    /// UI analysis (screenshot of web/app interface)
    UiAnalysis,
    
    /// Diagram/chart interpretation
    DiagramInterpretation,
    
    /// Voice command/query
    VoiceCommand,
    
    /// Audio transcription + analysis
    TranscriptionAndAnalysis,
    
    /// Screenshot-based debugging
    DebuggingFromScreenshot,
    
    /// Custom context
    Custom(String),
}

impl Default for RequestContext {
    fn default() -> Self {
        Self::Chat
    }
}

/// Multi-modal response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiModalResponse {
    /// Response ID
    pub id: String,
    
    /// Model used
    pub model: String,
    
    /// Generated text content
    pub text: String,
    
    /// Structured output (if requested)
    #[serde(default)]
    pub structured_output: Option<serde_json::Value>,
    
    /// Token usage
    #[serde(default)]
    pub usage: UsageInfo,
    
    /// Latency information
    #[serde(default)]
    pub latency_ms: f64,
    
    /// Processing details per modality
    #[serde(default)]
    pub processing_details: Vec<ModalityProcessingDetail>,
}

/// Token usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Per-modality processing detail
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModalityProcessingDetail {
    pub modality: Modality,
    pub processing_time_ms: f64,
    pub tokens_used: u32,
    pub model_used: Option<String>,
    pub confidence_score: Option<f64>,
}

/// Streaming chunk for multi-modal responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiModalStreamChunk {
    pub id: String,
    pub model: String,
    
    /// Text delta
    pub text_delta: Option<String>,
    
    /// Done signal
    pub done: bool,
    
    /// Final usage info (only present in last chunk)
    pub usage: Option<UsageInfo>,
}

/// Multi-modal router and processor
pub struct MultiModalService {
    vision_processor: Arc<dyn VisionProcessor>,
    audio_processor: Arc<dyn AudioProcessor>,
    encoder_manager: Arc<dyn EncoderManager>,
}

impl MultiModalService {
    pub fn new(
        vision_processor: Arc<dyn VisionProcessor>,
        audio_processor: Arc<dyn AudioProcessor>,
        encoder_manager: Arc<dyn EncoderManager>,
    ) -> Self {
        Self {
            vision_processor,
            audio_processor,
            encoder_manager,
        }
    }
    
    /// Process a multi-modal request
    #[instrument(skip(self), fields(session_id = %request.session_id))]
    pub async fn process_request(
        &self,
        request: MultiModalRequest,
    ) -> Result<MultiModalResponse> {
        let start = std::time::Instant::now();
        
        info!(
            session_id = %request.session_id,
            parts = request.parts.len(),
            model = %request.model,
            "Processing multi-modal request"
        );
        
        // Step 1: Validate and preprocess each modality
        let mut processed_parts = Vec::new();
        let mut processing_details = Vec::new();
        
        for (index, part) in request.parts.iter().enumerate() {
            match part.modality {
                Modality::Text => {
                    // Text doesn't need preprocessing
                    if let Some(ref text) = part.text {
                        processed_parts.push(ContentPart {
                            modality: Modality::Text,
                            text: Some(text.clone()),
                            ..part.clone()
                        });
                    }
                }
                
                Modality::Image => {
                    if let Some(ref img_data) = part.image_data {
                        debug!(index = index, "Processing image");
                        
                        let proc_start = Instant::now();
                        
                        // Encode/resize/optimize image
                        let processed_image = self.vision_processor
                            .process_image(img_data)
                            .await?;
                        
                        let processing_time = proc_start.elapsed().as_millis() as f64;
                        
                        processing_details.push(ModalityProcessingDetail {
                            modality: Modality::Image,
                            processing_time_ms: processing_time,
                            tokens_used: self.estimate_image_tokens(&processed_image),
                            model_used: None,
                            confidence_score: None,
                        });
                        
                        processed_parts.push(ContentPart {
                            modality: Modality::Image,
                            image_data: Some(processed_image),
                            ..part.clone()
                        });
                    }
                }
                
                Modality::Audio => {
                    if let Some(ref audio_data) = part.audio_data {
                        debug!(index = index, "Processing audio");
                        
                        let proc_start = Instant::now();
                        
                        // Transcribe audio to text
                        let transcription = self.audio_processor
                            .transcribe(audio_data)
                            .await?;
                        
                        let processing_time = proc_start.elapsed().as_millis() as f64;
                        
                        processing_details.push(ModalityProcessingDetail {
                            modality: Modality::Audio,
                            processing_time_ms: processing_time,
                            tokens_used: transcription.word_count() as u32 / 4, // Rough estimate
                            model_used: Some("whisper".to_string()),
                            confidence_score: Some(transcription.confidence),
                        });
                        
                        // Add transcribed text as a new part
                        processed_parts.push(ContentPart {
                            modality: Modality::Text,
                            text: Some(format!("[Audio Transcription]: {}", transcription.text)),
                            metadata: ContentMetadata {
                                language: audio_data.channels.and_then(|_| part.metadata.language.clone()),
                                ..Default::default()
                            },
                            ..ContentPart::default()
                        });
                    }
                }
                
                Modality::Video => {
                    // Video not yet supported - would need frame extraction
                    warn!(index = index, "Video modality not yet supported");
                }
            }
        }
        
        // Step 2: Build final request for LLM
        // (This would call the actual LLM provider)
        let latency_ms = start.elapsed().as_millis() as f64;
        
        Ok(MultiModalResponse {
            id: uuid::Uuid::new_v4().to_string(),
            model: request.model.clone(),
            text: "[Multi-modal response placeholder]".to_string(),
            structured_output: None,
            usage: UsageInfo {
                prompt_tokens: 100, // Placeholder
                completion_tokens: 50,
                total_tokens: 150,
            },
            latency_ms,
            processing_details,
        })
    }
    
    /// Estimate token count for an image
    fn estimate_image_tokens(&self, _image: &ImageData) -> u32 {
        // GPT-4V uses approximately:
        // - Low res: 85 tokens
        // - High res: depends on size (170 tokens per 512x512 tile)
        // This is a simplified estimate
        170
    }
}

/// Trait for vision/image processing
#[async_trait::async_trait]
pub trait VisionProcessor: Send + Sync {
    /// Process and optimize image for LLM input
    async fn process_image(&self, image: &ImageData) -> Result<ImageData>;
    
    /// Analyze image and extract features
    async fn analyze_image(&self, image: &ImageData) -> Result<ImageAnalysis>;
    
    /// Extract text from image (OCR)
    async fn ocr(&self, image: &ImageData) -> Result<String>;
}

/// Result of image analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAnalysis {
    /// Detected objects/classes
    pub objects: Vec<Detection>,
    
    /// Scene description
    pub description: String,
    
    /// Text detected via OCR
    pub extracted_text: Option<String>,
    
    /// UI elements detected (if applicable)
    pub ui_elements: Vec<UiElement>,
}

/// Object detection result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detection {
    pub label: String,
    pub confidence: f64,
    pub bounding_box: BoundingBox,
}

/// Bounding box coordinates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// UI element detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiElement {
    pub element_type: String, // button, input, text, etc.
    pub label: Option<String>,
    pub bounding_box: BoundingBox,
    pub attributes: std::collections::HashMap<String, String>,
}

/// Default vision processor implementation
pub struct DefaultVisionProcessor;

#[async_trait::async_trait]
impl VisionProcessor for DefaultVisionProcessor {
    async fn process_image(&self, image: &ImageData) -> Result<ImageData> {
        // In production, this would:
        // 1. Decode base64
        // 2. Resize if needed (max 2048x2048 for GPT-4V)
        // 3. Optimize compression
        // 4. Re-encode to base64
        
        // For now, return as-is (placeholder)
        Ok(image.clone())
    }
    
    async fn analyze_image(&self, image: &ImageData) -> Result<ImageAnalysis> {
        // Would use vision-language model here
        Ok(ImageAnalysis {
            objects: vec![],
            description: "[Image analysis not implemented]".to_string(),
            extracted_text: None,
            ui_elements: vec![],
        })
    }
    
    async fn ocr(&self, image: &ImageData) -> Result<String> {
        // Would use Tesseract or similar OCR engine
        Ok("[OCR not implemented]".to_string())
    }
}

/// Trait for audio processing
#[async_trait::async_trait]
pub trait AudioProcessor: Send + Sync {
    /// Transcribe audio to text
    async fn transcribe(&self, audio: &AudioData) -> Result<TranscriptionResult>;
    
    /// Analyze audio characteristics
    async fn analyze_audio(&self, audio: &AudioData) -> Result<AudioAnalysis>;
}

/// Transcription result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub text: String,
    pub language: Option<String>,
    pub confidence: f64,
    pub segments: Vec<TranscriptionSegment>,
}

/// Single transcription segment with timestamps
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionSegment {
    pub start_secs: f64,
    pub end_secs: f64,
    pub text: String,
    pub confidence: f64,
}

/// Audio analysis results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioAnalysis {
    pub duration_secs: f64,
    pub sample_rate: u32,
    pub channels: u8,
    pub detected_language: Option<String>,
    pub is_speech: bool,
    pub volume_level: f64,
}

/// Default audio processor using Whisper
pub struct WhisperAudioProcessor {
    whisper_model_path: String,
}

impl WhisperAudioProcessor {
    pub fn new(model_path: impl Into<String>) -> Self {
        Self {
            whisper_model_path: model_path.into(),
        }
    }
}

#[async_trait::async_trait]
impl AudioProcessor for WhisperAudioProcessor {
    async fn transcribe(&self, audio: &AudioData) -> Result<TranscriptionResult> {
        // Would call Whisper API/local model here
        // For now, return placeholder
        
        Ok(TranscriptionResult {
            text: "[Audio transcription not implemented]".to_string(),
            language: audio.channels.map(|_| "en".to_string()),
            confidence: 0.95,
            segments: vec![],
        })
    }
    
    async fn analyze_audio(&self, audio: &AudioData) -> Result<AudioAnalysis> {
        Ok(AudioAnalysis {
            duration_secs: audio.duration_secs.unwrap_or(10.0),
            sample_rate: audio.sample_rate.unwrap_or(16000),
            channels: audio.channels.unwrap_or(1),
            detected_language: None,
            is_speech: true,
            volume_level: 0.7,
        })
    }
}

/// Trait for managing encoders/embeddings
#[async_trait::async_trait]
pub trait EncoderManager: Send + Sync {
    /// Get embedding for text
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>>;
    
    /// Get embedding for image
    async fn embed_image(&self, image: &ImageData) -> Result<Vec<f32>>;
    
    /// Compute similarity between embeddings
    fn cosine_similarity(&self, a: &[f32], b: &[f32]) -> f64;
}

/// Default encoder manager using CLIP-like models
pub struct ClipEncoderManager;

#[async_trait::async_trait]
impl EncoderManager for ClipEncoderManager {
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        // Would call CLIP text encoder
        // Return dummy 512-dim vector
        Ok(vec![0.0f32; 512])
    }
    
    async fn embed_image(&self, _image: &ImageData) -> Result<Vec<f32>> {
        // Would call CLIP image encoder
        Ok(vec![0.0f32; 512])
    }
    
    fn cosine_similarity(&self, a: &[f32], b: &[f32]) -> f64 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        
        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        
        (dot_product / (norm_a * norm_b)) as f64
    }
}

/// Utility functions for encoding files to base64
pub mod encoding {
    use super::*;
    
    /// Read file and encode to base64
    pub async fn file_to_base64(path: impl AsRef<std::path::Path>) -> Result<(String, String)> {
        let path = path.as_ref();
        let mut file = tokio::fs::File::open(path).await.context("Failed to open file")?;
        
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await.context("Failed to read file")?;
        
        let base64 = STANDARD.encode(&buffer);
        let media_type = mime_guess::from_path(path)
            .first_or_octet_stream()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        
        Ok((base64, media_type))
    }
    
    /// Create ImageData from file path
    pub async fn image_from_file(path: impl AsRef<std::path::Path>) -> Result<ImageData> {
        let (base64, media_type) = file_to_base64(path).await?;
        
        // Try to get dimensions (requires image crate)
        let dimensions = get_image_dimensions(&base64).ok();
        
        Ok(ImageData {
            base64,
            media_type,
            dimensions,
            size_bytes: Some(base64.len() as u64),
        })
    }
    
    /// Create AudioData from file path
    pub async fn audio_from_file(path: impl AsRef<std::path::Path>) -> Result<AudioData> {
        let (base64, media_type) = file_to_base64(path).await?;
        
        Ok(AudioData {
            base64,
            media_type,
            duration_secs: None,
            sample_rate: None,
            channels: None,
        })
    }
    
    /// Get image dimensions from base64 data
    fn get_image_dimensions(_base64: &str) -> Result<(u32, u32)> {
        // Would use image crate to decode header
        // For now, return placeholder
        Ok((1024, 768))
    }
}

/// Helper trait for word count
trait WordCount {
    fn word_count(&self) -> usize;
}

impl WordCount for str {
    fn word_count(&self) -> usize {
        self.split_whitespace().count()
    }
}
